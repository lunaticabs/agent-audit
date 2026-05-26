use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{Value, json};
use sha3::{Digest, Keccak256};
use url::Url;

use crate::models::discovery::DependencyCandidate;
use crate::models::discovery::DependencyDiscoveryContext;
use crate::models::finding::{
    AddressCallResult, ChainBlockSnapshot, ChainCheckStatus, ChainlinkLatestRoundData,
    DependencyChainChecksArtifact, ExternalDependencySignal, FindingSeverity,
    FlashLoanSurfaceArtifact, FlashLoanSurfaceEntry, OracleCheckResult, OracleChecksArtifact,
    ProxyCheckResult, ProxyChecksArtifact, TargetFunctionSignal, TokenSurfaceMetadata,
};
use crate::models::identity::EvmAddress;
use crate::models::path::{RelativePath, WorkspaceRelPath};
use crate::models::run::RunTarget;
use crate::models::source::{ContractMetadata, DependencyRecord, SourceBundleArtifact};
use crate::workspace::paths;

use super::AuditPipelineService;

const EIP1967_IMPLEMENTATION_SLOT: &str =
    "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc";
const EIP1967_ADMIN_SLOT: &str =
    "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103";
const EIP1967_BEACON_SLOT: &str =
    "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50";

pub(super) struct DependencyChainArtifacts {
    pub summary: DependencyChainChecksArtifact,
    pub proxy: ProxyChecksArtifact,
    pub oracle: OracleChecksArtifact,
    pub flash: FlashLoanSurfaceArtifact,
}

#[derive(Clone)]
struct ChainSubject {
    address: EvmAddress,
    role: String,
    name: String,
    provider_is_proxy: bool,
    provider_implementation: Option<EvmAddress>,
    abi: Value,
    files: Vec<RelativePath>,
    provider_response_artifact: Option<WorkspaceRelPath>,
    discovery_file: Option<RelativePath>,
    hints: Vec<String>,
}

#[derive(Clone)]
struct OracleSubject {
    address: EvmAddress,
    role: String,
    name: String,
    internal_type: String,
    declared_type: String,
    source_files: Vec<RelativePath>,
    record: Option<ChainSubject>,
}

#[derive(Clone)]
struct FlashSurfaceSubject {
    address: EvmAddress,
    role: String,
    name: String,
    category: String,
    source_files: Vec<RelativePath>,
    abi_hints: Vec<String>,
    record: Option<ChainSubject>,
}

#[derive(Default)]
struct ChainExecutionStats {
    attempted: usize,
    succeeded: usize,
}

impl ChainExecutionStats {
    fn record_attempt(&mut self, ok: bool) {
        self.attempted += 1;
        if ok {
            self.succeeded += 1;
        }
    }

    fn status(&self) -> ChainCheckStatus {
        match (self.attempted, self.succeeded) {
            (0, _) => ChainCheckStatus::Executed,
            (_, 0) => ChainCheckStatus::CallFailed,
            (attempted, succeeded) if attempted == succeeded => ChainCheckStatus::Executed,
            _ => ChainCheckStatus::Partial,
        }
    }
}

struct RpcClient {
    url: Url,
    client: Client,
}

impl AuditPipelineService {
    pub(super) fn build_dependency_chain_artifacts(
        &self,
        bundle: &SourceBundleArtifact,
        fallback_target: &RunTarget,
    ) -> DependencyChainArtifacts {
        build_dependency_chain_artifacts(
            bundle,
            fallback_target,
            self.config.rpc_url.as_ref(),
            self.workspace.root(),
        )
    }
}

fn build_dependency_chain_artifacts(
    bundle: &SourceBundleArtifact,
    fallback_target: &RunTarget,
    rpc_url: Option<&Url>,
    workspace_root: &Path,
) -> DependencyChainArtifacts {
    if !bundle.is_fetched() {
        return skipped_chain_artifacts(
            fallback_target.clone(),
            ChainCheckStatus::SourceNotFetched,
            rpc_url.is_some(),
            "Skipped dependency chain checks because source fetching did not complete.",
        );
    }

    let rpc_client = rpc_url.and_then(|url| RpcClient::new(url).ok());
    let latest_block = rpc_client
        .as_ref()
        .and_then(|client| client.latest_block().ok());
    let proxy = build_proxy_checks(bundle, rpc_client.as_ref(), workspace_root);
    let oracle = build_oracle_checks(bundle, rpc_client.as_ref(), latest_block.clone());
    let flash = build_flash_loan_surface(bundle, rpc_client.as_ref());
    let summary_status = aggregate_chain_statuses(&[proxy.status, oracle.status, flash.status]);
    let mut summary = DependencyChainChecksArtifact::new(bundle.target.clone(), summary_status);
    summary.rpc_url_configured = rpc_url.is_some();
    summary.latest_block = latest_block;
    summary.proxy_checks_artifact = Some(WorkspaceRelPath::new(paths::PROXY_CHECKS));
    summary.oracle_checks_artifact = Some(WorkspaceRelPath::new(paths::ORACLE_CHECKS));
    summary.flash_loan_surface_artifact = Some(WorkspaceRelPath::new(paths::FLASH_LOAN_SURFACE));
    summary.summary_signals = aggregate_signals(&proxy.checks, &oracle.checks);
    summary.evidence_artifacts = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::DEPENDENCY_CHAIN_CHECKS),
        WorkspaceRelPath::new(paths::PROXY_CHECKS),
        WorkspaceRelPath::new(paths::ORACLE_CHECKS),
        WorkspaceRelPath::new(paths::FLASH_LOAN_SURFACE),
    ];
    if rpc_url.is_none() {
        summary.note =
            Some("RPC is not configured; wrote skipped chain-check artifacts for review.".into());
    }

    DependencyChainArtifacts {
        summary,
        proxy,
        oracle,
        flash,
    }
}

fn skipped_chain_artifacts(
    target: RunTarget,
    status: ChainCheckStatus,
    rpc_url_configured: bool,
    note: &str,
) -> DependencyChainArtifacts {
    let note = Some(note.to_string());
    let mut summary = DependencyChainChecksArtifact::new(target.clone(), status);
    summary.rpc_url_configured = rpc_url_configured;
    summary.note = note.clone();
    summary.proxy_checks_artifact = Some(WorkspaceRelPath::new(paths::PROXY_CHECKS));
    summary.oracle_checks_artifact = Some(WorkspaceRelPath::new(paths::ORACLE_CHECKS));
    summary.flash_loan_surface_artifact = Some(WorkspaceRelPath::new(paths::FLASH_LOAN_SURFACE));
    summary.evidence_artifacts = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::DEPENDENCY_CHAIN_CHECKS),
    ];

    let mut proxy = ProxyChecksArtifact::new(target.clone(), status);
    proxy.note = note.clone();
    proxy.evidence_artifacts = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::PROXY_CHECKS),
    ];

    let mut oracle = OracleChecksArtifact::new(target.clone(), status);
    oracle.note = note.clone();
    oracle.evidence_artifacts = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::ORACLE_CHECKS),
    ];

    let mut flash = FlashLoanSurfaceArtifact::new(target, status);
    flash.note = note;
    flash.evidence_artifacts = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::FLASH_LOAN_SURFACE),
    ];

    DependencyChainArtifacts {
        summary,
        proxy,
        oracle,
        flash,
    }
}

fn build_proxy_checks(
    bundle: &SourceBundleArtifact,
    rpc_client: Option<&RpcClient>,
    workspace_root: &Path,
) -> ProxyChecksArtifact {
    let status = if rpc_client.is_some() {
        ChainCheckStatus::Executed
    } else {
        ChainCheckStatus::RpcNotConfigured
    };
    let mut artifact = ProxyChecksArtifact::new(bundle.target.clone(), status);
    artifact.evidence_artifacts = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::PROXY_CHECKS),
    ];
    let mut stats = ChainExecutionStats::default();
    for subject in collect_proxy_subjects(bundle) {
        let mut implementation_slot_read = false;
        let mut result = ProxyCheckResult {
            address: subject.address.to_string(),
            role: subject.role.clone(),
            name: subject.name.clone(),
            provider_is_proxy: subject.provider_is_proxy,
            provider_implementation: subject
                .provider_implementation
                .as_ref()
                .map(ToString::to_string),
            evidence_artifacts: proxy_evidence(&subject),
            ..ProxyCheckResult::default()
        };
        if let Some(client) = rpc_client {
            let impl_slot = client.get_storage_at(&subject.address, EIP1967_IMPLEMENTATION_SLOT);
            implementation_slot_read = impl_slot.is_ok();
            stats.record_attempt(impl_slot.is_ok());
            result.eip1967_implementation = impl_slot
                .ok()
                .and_then(|word| decode_address_from_word(&word))
                .map(|address| address.to_string());

            let admin_slot = client.get_storage_at(&subject.address, EIP1967_ADMIN_SLOT);
            stats.record_attempt(admin_slot.is_ok());
            result.eip1967_admin = admin_slot
                .ok()
                .and_then(|word| decode_address_from_word(&word))
                .map(|address| address.to_string());

            let beacon_slot = client.get_storage_at(&subject.address, EIP1967_BEACON_SLOT);
            stats.record_attempt(beacon_slot.is_ok());
            result.eip1967_beacon = beacon_slot
                .ok()
                .and_then(|word| decode_address_from_word(&word))
                .map(|address| address.to_string());

            if supports_function(
                &subject.abi,
                workspace_root,
                &subject.files,
                "implementation",
            ) {
                result.implementation_call = Some(address_call(
                    client,
                    &subject.address,
                    "implementation()",
                    &mut stats,
                ));
            }
            if supports_function(&subject.abi, workspace_root, &subject.files, "admin") {
                result.admin_call = Some(address_call(
                    client,
                    &subject.address,
                    "admin()",
                    &mut stats,
                ));
            }
            if supports_function(&subject.abi, workspace_root, &subject.files, "owner") {
                result.owner_call = Some(address_call(
                    client,
                    &subject.address,
                    "owner()",
                    &mut stats,
                ));
            }
            if supports_function(
                &subject.abi,
                workspace_root,
                &subject.files,
                "proxiableUUID",
            ) {
                let call = client.eth_call(&subject.address, &encode_call("proxiableUUID()", &[]));
                stats.record_attempt(call.is_ok());
                if let Ok(output) = call
                    && let Some(bytes) = decode_hex_bytes(&output)
                    && bytes.len() >= 32
                {
                    result.proxiable_uuid = Some(format!("0x{}", hex_lower(&bytes[..32])));
                }
            }
        }
        if rpc_client.is_some() {
            result.signals = proxy_signals(&result, implementation_slot_read);
        }
        artifact.checks.push(result);
    }
    artifact.status = if rpc_client.is_some() {
        stats.status()
    } else {
        ChainCheckStatus::RpcNotConfigured
    };
    if rpc_client.is_none() {
        artifact.note =
            Some("RPC is not configured; proxy slot reads and ABI calls were skipped.".into());
    }
    artifact
}

fn build_oracle_checks(
    bundle: &SourceBundleArtifact,
    rpc_client: Option<&RpcClient>,
    latest_block: Option<ChainBlockSnapshot>,
) -> OracleChecksArtifact {
    let status = if rpc_client.is_some() {
        ChainCheckStatus::Executed
    } else {
        ChainCheckStatus::RpcNotConfigured
    };
    let mut artifact = OracleChecksArtifact::new(bundle.target.clone(), status);
    artifact.latest_block = latest_block.clone();
    artifact.evidence_artifacts = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::ORACLE_CHECKS),
    ];
    let mut stats = ChainExecutionStats::default();
    for subject in collect_oracle_subjects(bundle) {
        let mut result = OracleCheckResult {
            address: subject.address.to_string(),
            role: subject.role.clone(),
            name: subject.name.clone(),
            candidate_hints: oracle_candidate_hints(&subject),
            evidence_artifacts: oracle_evidence(&subject),
            ..OracleCheckResult::default()
        };
        if let Some(client) = rpc_client {
            let decimals_call = client.eth_call(&subject.address, &encode_call("decimals()", &[]));
            if decimals_call.is_err() {
                result.failed_reads.push("decimals".into());
            }
            stats.record_attempt(decimals_call.is_ok());
            result.decimals = decimals_call
                .ok()
                .and_then(|value| decode_hex_bytes(&value))
                .and_then(|bytes| decode_uint_u64(&bytes))
                .map(|value| value as u8);

            let description_call =
                client.eth_call(&subject.address, &encode_call("description()", &[]));
            if description_call.is_err() {
                result.failed_reads.push("description".into());
            }
            stats.record_attempt(description_call.is_ok());
            result.description = description_call
                .ok()
                .and_then(|value| decode_string_output(&value));

            let version_call = client.eth_call(&subject.address, &encode_call("version()", &[]));
            if version_call.is_err() {
                result.failed_reads.push("version".into());
            }
            stats.record_attempt(version_call.is_ok());
            result.version = version_call
                .ok()
                .and_then(|value| decode_hex_bytes(&value))
                .and_then(|bytes| decode_uint_u64(&bytes));

            let latest_round_call =
                client.eth_call(&subject.address, &encode_call("latestRoundData()", &[]));
            if latest_round_call.is_err() {
                result.failed_reads.push("latestRoundData".into());
            }
            stats.record_attempt(latest_round_call.is_ok());
            let latest_round = decode_latest_round_data_response(latest_round_call);
            if let Some(data) = latest_round {
                result.staleness_seconds = latest_block
                    .as_ref()
                    .and_then(|block| block.timestamp.checked_sub(data.updated_at));
                result.latest_round_data = Some(data);
            }
        }
        if rpc_client.is_some() {
            result.signals = oracle_signals(&result, latest_block.as_ref());
        }
        artifact.checks.push(result);
    }
    artifact.status = if rpc_client.is_some() {
        stats.status()
    } else {
        ChainCheckStatus::RpcNotConfigured
    };
    if rpc_client.is_none() {
        artifact.note =
            Some("RPC is not configured; Chainlink-compatible oracle reads were skipped.".into());
    }
    artifact
}

fn build_flash_loan_surface(
    bundle: &SourceBundleArtifact,
    rpc_client: Option<&RpcClient>,
) -> FlashLoanSurfaceArtifact {
    let status = if rpc_client.is_some() {
        ChainCheckStatus::Executed
    } else {
        ChainCheckStatus::RpcNotConfigured
    };
    let mut artifact = FlashLoanSurfaceArtifact::new(bundle.target.clone(), status);
    artifact.evidence_artifacts = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::FLASH_LOAN_SURFACE),
    ];
    artifact.target_function_signals = target_function_signals(bundle);
    let relevant_functions = target_function_names_by_category(&artifact.target_function_signals);
    let mut stats = ChainExecutionStats::default();
    for subject in collect_flash_surface_subjects(bundle) {
        let mut entry = FlashLoanSurfaceEntry {
            address: subject.address.to_string(),
            role: subject.role.clone(),
            name: subject.name.clone(),
            source_locations: subject
                .source_files
                .iter()
                .map(ToString::to_string)
                .collect(),
            abi_hints: subject.abi_hints.clone(),
            target_function_matches: relevant_target_functions(
                &subject.category,
                &relevant_functions,
            ),
            evidence_artifacts: flash_surface_evidence(&subject),
            ..FlashLoanSurfaceEntry::default()
        };
        if let Some(client) = rpc_client
            && subject.category == "token"
        {
            let token = token_surface_metadata(
                client,
                &subject.address,
                &bundle.target.address,
                &mut stats,
            );
            if token.symbol.is_some()
                || token.decimals.is_some()
                || token.total_supply.is_some()
                || token.balance_of_target.is_some()
            {
                entry.token_metadata = Some(token);
            }
        }
        artifact.dependencies.push(entry);
    }
    artifact.status = if rpc_client.is_some() {
        match (stats.attempted, stats.succeeded) {
            (0, _) => ChainCheckStatus::Executed,
            _ => stats.status(),
        }
    } else {
        ChainCheckStatus::RpcNotConfigured
    };
    if rpc_client.is_none() {
        artifact.note = Some(
            "RPC is not configured; token metadata reads were skipped while source-derived surface mapping was retained."
                .into(),
        );
    }
    artifact
}

fn collect_proxy_subjects(bundle: &SourceBundleArtifact) -> Vec<ChainSubject> {
    let mut seen = BTreeSet::new();
    let mut subjects = Vec::new();
    let target_subject = target_chain_subject(bundle);
    seen.insert(target_subject.address.as_lowercase());
    subjects.push(target_subject);
    for record in bundle
        .related_contracts
        .iter()
        .chain(flatten_dependency_records(&bundle.dependencies))
    {
        let subject = subject_from_record(record);
        if !is_proxy_subject(record, &subject) {
            continue;
        }
        if seen.insert(subject.address.as_lowercase()) {
            subjects.push(subject);
        }
    }
    subjects
}

fn target_chain_subject(bundle: &SourceBundleArtifact) -> ChainSubject {
    ChainSubject {
        address: bundle.target.address.clone(),
        role: "target".into(),
        name: bundle
            .contract
            .as_ref()
            .map(|contract| contract.name.clone())
            .unwrap_or_else(|| "target".into()),
        provider_is_proxy: bundle
            .contract
            .as_ref()
            .is_some_and(|contract| contract.proxy),
        provider_implementation: bundle
            .contract
            .as_ref()
            .and_then(|contract| contract.implementation.clone()),
        abi: bundle.abi.clone(),
        files: bundle.files.iter().map(|file| file.path.clone()).collect(),
        provider_response_artifact: Some(WorkspaceRelPath::new(paths::SOURCE_PROVIDER_RESPONSE)),
        discovery_file: bundle
            .contract
            .as_ref()
            .and_then(|contract| contract.file_name.clone()),
        hints: Vec::new(),
    }
}

fn collect_oracle_subjects(bundle: &SourceBundleArtifact) -> Vec<OracleSubject> {
    let mut merged = BTreeMap::<String, OracleSubject>::new();
    let record_by_address = dependency_record_index(bundle);
    if let Some(discovery) = bundle.dependency_discovery.as_ref() {
        for candidate in &discovery.merged_candidates {
            if !candidate_looks_like_oracle(candidate) {
                continue;
            }
            let key = candidate.address.as_lowercase();
            let record = record_by_address
                .get(&key)
                .map(|record| subject_from_record(record));
            merged.entry(key).or_insert_with(|| OracleSubject {
                address: candidate.address.clone(),
                role: candidate.role.clone(),
                name: candidate.name.clone(),
                internal_type: candidate.internal_type.clone(),
                declared_type: candidate.declared_type.clone(),
                source_files: candidate.file.clone().into_iter().collect(),
                record,
            });
        }
    }
    for record in flatten_dependency_records(&bundle.dependencies) {
        if !record_looks_like_oracle(record) {
            continue;
        }
        let key = record.address.as_lowercase();
        let subject = merged.entry(key).or_insert_with(|| OracleSubject {
            address: record.address.clone(),
            role: record.role.clone(),
            name: record.name.clone(),
            internal_type: record
                .discovery
                .as_ref()
                .map(|ctx| ctx.internal_type.clone())
                .unwrap_or_default(),
            declared_type: String::new(),
            source_files: record_source_files(record),
            record: Some(subject_from_record(record)),
        });
        if subject.name.is_empty() {
            subject.name.clone_from(&record.name);
        }
        if subject.role.is_empty() {
            subject.role.clone_from(&record.role);
        }
    }
    merged.into_values().collect()
}

fn collect_flash_surface_subjects(bundle: &SourceBundleArtifact) -> Vec<FlashSurfaceSubject> {
    let mut merged = BTreeMap::<String, FlashSurfaceSubject>::new();
    let record_by_address = dependency_record_index(bundle);
    if let Some(discovery) = bundle.dependency_discovery.as_ref() {
        for candidate in &discovery.merged_candidates {
            let category = flash_surface_category(
                &candidate.role,
                &candidate.name,
                &candidate.internal_type,
                &candidate.declared_type,
            );
            let key = candidate.address.as_lowercase();
            let record = record_by_address
                .get(&key)
                .map(|record| subject_from_record(record));
            merged.entry(key).or_insert_with(|| FlashSurfaceSubject {
                address: candidate.address.clone(),
                role: candidate.role.clone(),
                name: candidate.name.clone(),
                category,
                source_files: candidate.file.clone().into_iter().collect(),
                abi_hints: record
                    .as_ref()
                    .map(|subject| abi_function_hints(&subject.abi))
                    .unwrap_or_default(),
                record,
            });
        }
    }
    for record in flatten_dependency_records(&bundle.dependencies) {
        let key = record.address.as_lowercase();
        let subject = merged.entry(key).or_insert_with(|| FlashSurfaceSubject {
            address: record.address.clone(),
            role: record.role.clone(),
            name: record.name.clone(),
            category: flash_surface_category(
                &record.role,
                &record.name,
                &record
                    .discovery
                    .as_ref()
                    .map(|ctx| ctx.internal_type.clone())
                    .unwrap_or_default(),
                "",
            ),
            source_files: record_source_files(record),
            abi_hints: abi_function_hints(&record.abi),
            record: Some(subject_from_record(record)),
        });
        if subject.abi_hints.is_empty() {
            subject.abi_hints = abi_function_hints(&record.abi);
        }
    }
    merged.into_values().collect()
}

fn dependency_record_index(bundle: &SourceBundleArtifact) -> BTreeMap<String, &DependencyRecord> {
    let mut index = BTreeMap::new();
    for record in flatten_dependency_records(&bundle.related_contracts) {
        index.insert(record.address.as_lowercase(), record);
    }
    for record in flatten_dependency_records(&bundle.dependencies) {
        index.insert(record.address.as_lowercase(), record);
    }
    index
}

fn flatten_dependency_records(records: &[DependencyRecord]) -> Vec<&DependencyRecord> {
    let mut flat = Vec::new();
    for record in records {
        flat.push(record);
        flat.extend(flatten_dependency_records(&record.related_contracts));
    }
    flat
}

fn subject_from_record(record: &DependencyRecord) -> ChainSubject {
    ChainSubject {
        address: record.address.clone(),
        role: record.role.clone(),
        name: record.name.clone(),
        provider_is_proxy: record
            .contract
            .as_ref()
            .is_some_and(|contract| contract.proxy),
        provider_implementation: record
            .contract
            .as_ref()
            .and_then(|contract| contract.implementation.clone()),
        abi: record.abi.clone(),
        files: record.files.iter().map(|file| file.path.clone()).collect(),
        provider_response_artifact: record.provider_response_artifact.clone(),
        discovery_file: record.discovery.as_ref().and_then(|ctx| ctx.file.clone()),
        hints: subject_hints(record.discovery.as_ref(), record.contract.as_ref()),
    }
}

fn subject_hints(
    discovery: Option<&DependencyDiscoveryContext>,
    contract: Option<&ContractMetadata>,
) -> Vec<String> {
    let mut hints = Vec::new();
    if let Some(ctx) = discovery {
        if !ctx.internal_type.is_empty() {
            hints.push(ctx.internal_type.clone());
        }
        if !ctx.solidity_type.is_empty() {
            hints.push(ctx.solidity_type.clone());
        }
    }
    if let Some(contract) = contract
        && !contract.name.is_empty()
    {
        hints.push(contract.name.clone());
    }
    hints
}

fn is_proxy_subject(record: &DependencyRecord, subject: &ChainSubject) -> bool {
    subject.provider_is_proxy
        || subject.provider_implementation.is_some()
        || record.role == "implementation"
        || record.name.to_lowercase().contains("proxy")
}

fn candidate_looks_like_oracle(candidate: &DependencyCandidate) -> bool {
    looks_like_oracle(
        &candidate.role,
        &candidate.name,
        &candidate.internal_type,
        &candidate.declared_type,
    )
}

fn record_looks_like_oracle(record: &DependencyRecord) -> bool {
    looks_like_oracle(
        &record.role,
        &record.name,
        &record
            .discovery
            .as_ref()
            .map(|ctx| ctx.internal_type.clone())
            .unwrap_or_default(),
        "",
    )
}

fn looks_like_oracle(role: &str, name: &str, internal_type: &str, declared_type: &str) -> bool {
    let haystack = format!("{role} {name} {internal_type} {declared_type}")
        .replace('_', " ")
        .to_lowercase();
    ["oracle", "price", "aggregator", "feed"]
        .iter()
        .any(|needle| haystack.contains(needle))
}

fn flash_surface_category(
    role: &str,
    name: &str,
    internal_type: &str,
    declared_type: &str,
) -> String {
    let haystack = format!("{role} {name} {internal_type} {declared_type}")
        .replace('_', "")
        .to_lowercase();
    for (needle, category) in [
        ("token", "token"),
        ("erc20", "token"),
        ("router", "router"),
        ("pair", "pair"),
        ("pool", "pool"),
        ("vault", "vault"),
        ("lending", "lending"),
        ("borrow", "lending"),
        ("oracle", "oracle"),
        ("price", "oracle"),
        ("aggregator", "oracle"),
        ("admin", "access-control"),
        ("owner", "access-control"),
        ("guardian", "access-control"),
    ] {
        if haystack.contains(needle) {
            return category.to_string();
        }
    }
    "unknown".to_string()
}

fn proxy_signals(
    result: &ProxyCheckResult,
    implementation_slot_read: bool,
) -> Vec<ExternalDependencySignal> {
    let mut signals = Vec::new();
    if result.provider_is_proxy
        && implementation_slot_read
        && result.eip1967_implementation.is_none()
    {
        signals.push(signal(
            "provider_proxy_without_eip1967_impl",
            FindingSeverity::Medium,
            "Provider metadata marks this contract as a proxy, but the EIP-1967 implementation slot decoded to zero.",
            Some(result.address.clone()),
            None,
            &result.evidence_artifacts,
        ));
    }
    if result.provider_implementation.is_some()
        && result.eip1967_implementation.is_some()
        && result
            .provider_implementation
            .as_deref()
            .map(str::to_ascii_lowercase)
            != result
                .eip1967_implementation
                .as_deref()
                .map(str::to_ascii_lowercase)
    {
        signals.push(signal(
            "eip1967_impl_differs_from_provider",
            FindingSeverity::Medium,
            "The provider implementation address differs from the EIP-1967 implementation slot value.",
            Some(result.address.clone()),
            None,
            &result.evidence_artifacts,
        ));
    }
    if result.eip1967_admin.is_some()
        || result
            .admin_call
            .as_ref()
            .and_then(|call| call.address.as_ref())
            .is_some()
    {
        signals.push(signal(
            "admin_present",
            FindingSeverity::Low,
            "Proxy admin state is populated and should be reviewed for upgrade authority assumptions.",
            Some(result.address.clone()),
            None,
            &result.evidence_artifacts,
        ));
    }
    if result.eip1967_beacon.is_some() {
        signals.push(signal(
            "beacon_present",
            FindingSeverity::Low,
            "Beacon slot is populated; review beacon upgrade authority and implementation resolution.",
            Some(result.address.clone()),
            None,
            &result.evidence_artifacts,
        ));
    }
    if result.proxiable_uuid.is_some() {
        signals.push(signal(
            "uups_hint_present",
            FindingSeverity::Low,
            "proxiableUUID() responded, which suggests a UUPS-style implementation surface.",
            Some(result.address.clone()),
            None,
            &result.evidence_artifacts,
        ));
    }
    signals
}

fn oracle_signals(
    result: &OracleCheckResult,
    latest_block: Option<&ChainBlockSnapshot>,
) -> Vec<ExternalDependencySignal> {
    let mut signals = Vec::new();
    if !result.failed_reads.is_empty() {
        signals.push(signal(
            "oracle_standard_read_failed",
            FindingSeverity::Low,
            format!(
                "One or more Chainlink-compatible oracle reads failed: {}.",
                result.failed_reads.join(", ")
            ),
            Some(result.address.clone()),
            None,
            &result.evidence_artifacts,
        ));
    }
    if result.decimals.is_none()
        && result.description.is_none()
        && result.version.is_none()
        && result.latest_round_data.is_none()
        && result.failed_reads.len() < 4
    {
        signals.push(signal(
            "oracle_role_without_standard_interface_response",
            FindingSeverity::Medium,
            "This dependency looks oracle-like, but standard Chainlink-compatible reads returned no decodable data.",
            Some(result.address.clone()),
            None,
            &result.evidence_artifacts,
        ));
    }
    if let Some(data) = result.latest_round_data.as_ref() {
        if data.updated_at == 0 {
            signals.push(signal(
                "oracle_zero_timestamp",
                FindingSeverity::High,
                "latestRoundData().updatedAt returned zero.",
                Some(result.address.clone()),
                None,
                &result.evidence_artifacts,
            ));
        }
        if is_non_positive_signed_decimal(&data.answer) {
            signals.push(signal(
                "oracle_non_positive_answer",
                FindingSeverity::Medium,
                "latestRoundData().answer is non-positive.",
                Some(result.address.clone()),
                None,
                &result.evidence_artifacts,
            ));
        }
        if let Some(staleness) = result.staleness_seconds
            && latest_block.is_some()
        {
            signals.push(signal(
                "oracle_staleness_observed",
                FindingSeverity::Low,
                format!(
                    "Oracle data trails the latest block timestamp by {staleness} seconds; no protocol heartbeat threshold was inferred."
                ),
                Some(result.address.clone()),
                None,
                &result.evidence_artifacts,
            ));
        }
    }
    if result
        .decimals
        .is_some_and(|decimals| !(6..=18).contains(&decimals))
    {
        signals.push(signal(
            "oracle_unusual_decimals",
            FindingSeverity::Low,
            "Oracle decimals are outside the common 6-18 range.",
            Some(result.address.clone()),
            None,
            &result.evidence_artifacts,
        ));
    }
    signals
}

fn signal(
    signal: impl Into<String>,
    severity: FindingSeverity,
    summary: impl Into<String>,
    address: Option<String>,
    location: Option<String>,
    evidence_artifacts: &[WorkspaceRelPath],
) -> ExternalDependencySignal {
    ExternalDependencySignal {
        signal: signal.into(),
        severity,
        summary: summary.into(),
        address,
        location,
        evidence_artifacts: evidence_artifacts.to_vec(),
    }
}

fn proxy_evidence(subject: &ChainSubject) -> Vec<WorkspaceRelPath> {
    let mut evidence = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::PROXY_CHECKS),
    ];
    extend_source_evidence(&mut evidence, &subject.files);
    if let Some(path) = subject.provider_response_artifact.as_ref() {
        evidence.push(path.clone());
    }
    if let Some(path) = subject.discovery_file.as_ref() {
        evidence.push(WorkspaceRelPath::new(format!("sources/{path}")));
    }
    dedupe_evidence(&mut evidence);
    evidence
}

fn oracle_evidence(subject: &OracleSubject) -> Vec<WorkspaceRelPath> {
    let mut evidence = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::ORACLE_CHECKS),
    ];
    extend_source_evidence(&mut evidence, &subject.source_files);
    if let Some(record) = subject.record.as_ref() {
        extend_source_evidence(&mut evidence, &record.files);
        if let Some(path) = record.provider_response_artifact.as_ref() {
            evidence.push(path.clone());
        }
    }
    dedupe_evidence(&mut evidence);
    evidence
}

fn flash_surface_evidence(subject: &FlashSurfaceSubject) -> Vec<WorkspaceRelPath> {
    let mut evidence = vec![
        WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
        WorkspaceRelPath::new(paths::FLASH_LOAN_SURFACE),
    ];
    extend_source_evidence(&mut evidence, &subject.source_files);
    if let Some(record) = subject.record.as_ref() {
        extend_source_evidence(&mut evidence, &record.files);
    }
    dedupe_evidence(&mut evidence);
    evidence
}

fn record_source_files(record: &DependencyRecord) -> Vec<RelativePath> {
    let mut files = record
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    if let Some(path) = record.discovery.as_ref().and_then(|ctx| ctx.file.clone())
        && !files.iter().any(|item| item == &path)
    {
        files.push(path);
    }
    files
}

fn extend_source_evidence(evidence: &mut Vec<WorkspaceRelPath>, files: &[RelativePath]) {
    for file in files {
        evidence.push(WorkspaceRelPath::new(format!("sources/{file}")));
    }
}

fn dedupe_evidence(evidence: &mut Vec<WorkspaceRelPath>) {
    let mut seen = BTreeSet::new();
    evidence.retain(|path| seen.insert(path.as_str().to_string()));
}

fn aggregate_signals(
    proxy_checks: &[ProxyCheckResult],
    oracle_checks: &[OracleCheckResult],
) -> Vec<ExternalDependencySignal> {
    let mut signals = Vec::new();
    for check in proxy_checks {
        signals.extend(check.signals.clone());
    }
    for check in oracle_checks {
        signals.extend(check.signals.clone());
    }
    signals
}

fn aggregate_chain_statuses(statuses: &[ChainCheckStatus]) -> ChainCheckStatus {
    if statuses
        .iter()
        .all(|status| *status == ChainCheckStatus::RpcNotConfigured)
    {
        return ChainCheckStatus::RpcNotConfigured;
    }
    if statuses
        .iter()
        .all(|status| *status == ChainCheckStatus::SourceNotFetched)
    {
        return ChainCheckStatus::SourceNotFetched;
    }
    if statuses
        .iter()
        .all(|status| *status == ChainCheckStatus::CallFailed)
    {
        return ChainCheckStatus::CallFailed;
    }
    if statuses.iter().any(|status| {
        matches!(
            status,
            ChainCheckStatus::Partial | ChainCheckStatus::CallFailed
        )
    }) {
        return ChainCheckStatus::Partial;
    }
    if statuses.contains(&ChainCheckStatus::RpcNotConfigured) {
        return ChainCheckStatus::Partial;
    }
    ChainCheckStatus::Executed
}

fn address_call(
    client: &RpcClient,
    address: &EvmAddress,
    signature: &str,
    stats: &mut ChainExecutionStats,
) -> AddressCallResult {
    let output = client.eth_call(address, &encode_call(signature, &[]));
    stats.record_attempt(output.is_ok());
    match output {
        Ok(output) => AddressCallResult {
            status: ChainCheckStatus::Executed,
            address: decode_hex_bytes(&output)
                .and_then(|bytes| decode_address_from_bytes(&bytes))
                .map(|address| address.to_string()),
            error: None,
        },
        Err(error) => AddressCallResult {
            status: ChainCheckStatus::CallFailed,
            address: None,
            error: Some(error),
        },
    }
}

fn token_surface_metadata(
    client: &RpcClient,
    token: &EvmAddress,
    target: &EvmAddress,
    stats: &mut ChainExecutionStats,
) -> TokenSurfaceMetadata {
    let mut metadata = TokenSurfaceMetadata::default();

    let symbol = client.eth_call(token, &encode_call("symbol()", &[]));
    stats.record_attempt(symbol.is_ok());
    metadata.symbol = symbol.ok().and_then(|value| decode_string_output(&value));

    let decimals = client.eth_call(token, &encode_call("decimals()", &[]));
    stats.record_attempt(decimals.is_ok());
    metadata.decimals = decimals
        .ok()
        .and_then(|value| decode_hex_bytes(&value))
        .and_then(|bytes| decode_uint_u64(&bytes))
        .map(|value| value as u8);

    let total_supply = client.eth_call(token, &encode_call("totalSupply()", &[]));
    stats.record_attempt(total_supply.is_ok());
    metadata.total_supply = total_supply
        .ok()
        .and_then(|value| decode_hex_bytes(&value))
        .map(|bytes| decode_uint_decimal(&bytes));

    let balance_of = client.eth_call(
        token,
        &encode_call("balanceOf(address)", &[AbiArg::Address(target.clone())]),
    );
    stats.record_attempt(balance_of.is_ok());
    metadata.balance_of_target = balance_of
        .ok()
        .and_then(|value| decode_hex_bytes(&value))
        .map(|bytes| decode_uint_decimal(&bytes));

    metadata
}

fn target_function_signals(bundle: &SourceBundleArtifact) -> Vec<TargetFunctionSignal> {
    let Some(abi) = bundle.abi.as_array() else {
        return Vec::new();
    };
    let mut signals = Vec::new();
    for item in abi {
        if item.get("type").and_then(Value::as_str) != Some("function") {
            continue;
        }
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let inputs = item
            .get("inputs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let signature = format!(
            "{}({})",
            name,
            inputs
                .iter()
                .filter_map(|input| input.get("type").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(",")
        );
        let tags = function_tags(name);
        if tags.len() < 2 {
            continue;
        }
        signals.push(TargetFunctionSignal {
            name: name.to_string(),
            selector: format!("0x{}", hex_lower(&selector(signature.as_bytes()))),
            tags,
        });
    }
    signals
}

fn target_function_names_by_category(
    signals: &[TargetFunctionSignal],
) -> BTreeMap<String, Vec<String>> {
    let mut by_category = BTreeMap::<String, Vec<String>>::new();
    for signal in signals {
        for tag in &signal.tags {
            by_category
                .entry(tag.clone())
                .or_default()
                .push(signal.name.clone());
        }
    }
    by_category
}

fn relevant_target_functions(
    category: &str,
    by_category: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let tags = match category {
        "token" => vec!["token_movement", "mint_burn", "accounting_update"],
        "oracle" => vec!["oracle_read", "accounting_update"],
        "router" | "pair" | "pool" | "vault" => {
            vec!["swap", "token_movement", "accounting_update"]
        }
        "lending" => vec!["borrow_repay", "token_movement", "accounting_update"],
        _ => vec!["accounting_update"],
    };
    let mut functions = Vec::new();
    let mut seen = BTreeSet::new();
    for tag in tags {
        if let Some(items) = by_category.get(tag) {
            for item in items {
                if seen.insert(item.clone()) {
                    functions.push(item.clone());
                }
            }
        }
    }
    functions
}

fn function_tags(name: &str) -> Vec<String> {
    let lowered = name.replace('_', "").to_lowercase();
    let mut tags = Vec::new();
    for (needle, tag) in [
        ("oracle", "oracle_read"),
        ("price", "oracle_read"),
        ("swap", "swap"),
        ("mint", "mint_burn"),
        ("burn", "mint_burn"),
        ("borrow", "borrow_repay"),
        ("repay", "borrow_repay"),
        ("flash", "borrow_repay"),
        ("withdraw", "token_movement"),
        ("deposit", "token_movement"),
        ("transfer", "token_movement"),
        ("claim", "accounting_update"),
        ("rebalance", "accounting_update"),
        ("update", "accounting_update"),
        ("settle", "accounting_update"),
        ("liquid", "accounting_update"),
    ] {
        if lowered.contains(needle) && !tags.iter().any(|item| item == tag) {
            tags.push(tag.to_string());
        }
    }
    tags
}

fn oracle_candidate_hints(subject: &OracleSubject) -> Vec<String> {
    let mut hints = Vec::new();
    if !subject.role.is_empty() {
        hints.push(subject.role.clone());
    }
    if !subject.internal_type.is_empty() {
        hints.push(subject.internal_type.clone());
    }
    if !subject.declared_type.is_empty() {
        hints.push(subject.declared_type.clone());
    }
    if let Some(record) = subject.record.as_ref() {
        hints.extend(record.hints.clone());
    }
    hints.sort();
    hints.dedup();
    hints
}

fn supports_function(
    abi: &Value,
    workspace_root: &Path,
    files: &[RelativePath],
    function_name: &str,
) -> bool {
    if abi_has_function(abi, function_name) {
        return true;
    }
    files.iter().any(|file| {
        let path = workspace_root.join("sources").join(file.as_str());
        fs::read_to_string(path)
            .map(|text| text.contains(&format!("function {function_name}(")))
            .unwrap_or(false)
    })
}

fn abi_has_function(abi: &Value, function_name: &str) -> bool {
    abi.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function")
                && item.get("name").and_then(Value::as_str) == Some(function_name)
        })
    })
}

fn abi_function_hints(abi: &Value) -> Vec<String> {
    let mut hints = Vec::new();
    let Some(items) = abi.as_array() else {
        return hints;
    };
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("function") {
            continue;
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            hints.push(name.to_string());
        }
    }
    hints.sort();
    hints.dedup();
    hints
}

impl RpcClient {
    fn new(url: &Url) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            url: url.clone(),
            client,
        })
    }

    fn latest_block(&self) -> Result<ChainBlockSnapshot, String> {
        let response = self.request("eth_getBlockByNumber", json!(["latest", false]))?;
        let number = response
            .get("number")
            .and_then(Value::as_str)
            .ok_or_else(|| "latest block payload is missing number".to_string())?;
        let timestamp = response
            .get("timestamp")
            .and_then(Value::as_str)
            .ok_or_else(|| "latest block payload is missing timestamp".to_string())
            .and_then(parse_hex_u64)?;
        Ok(ChainBlockSnapshot {
            block_number: number.to_string(),
            timestamp,
        })
    }

    fn get_storage_at(&self, address: &EvmAddress, slot: &str) -> Result<String, String> {
        self.request(
            "eth_getStorageAt",
            json!([address.as_str(), slot, "latest"]),
        )
        .and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| "eth_getStorageAt returned a non-string result".to_string())
        })
    }

    fn eth_call(&self, address: &EvmAddress, data: &str) -> Result<String, String> {
        self.request(
            "eth_call",
            json!([{ "to": address.as_str(), "data": data }, "latest"]),
        )
        .and_then(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| "eth_call returned a non-string result".to_string())
        })
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let response = self
            .client
            .post(self.url.clone())
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1u64,
                "method": method,
                "params": params,
            }))
            .send()
            .map_err(|error| error.to_string())?;
        let payload: Value = response.json().map_err(|error| error.to_string())?;
        if let Some(error) = payload.get("error") {
            return Err(error.to_string());
        }
        payload
            .get("result")
            .cloned()
            .ok_or_else(|| format!("JSON-RPC response for {method} is missing result"))
    }
}

enum AbiArg {
    Address(EvmAddress),
}

fn encode_call(signature: &str, args: &[AbiArg]) -> String {
    let mut out = selector(signature.as_bytes()).to_vec();
    for arg in args {
        match arg {
            AbiArg::Address(address) => out.extend(encode_address_word(address)),
        }
    }
    format!("0x{}", hex_lower(&out))
}

fn selector(signature: &[u8]) -> [u8; 4] {
    let digest = Keccak256::digest(signature);
    [digest[0], digest[1], digest[2], digest[3]]
}

fn encode_address_word(address: &EvmAddress) -> [u8; 32] {
    let mut word = [0u8; 32];
    let raw = hex_to_fixed_20(address.as_str()).unwrap_or([0u8; 20]);
    word[12..32].copy_from_slice(&raw);
    word
}

fn hex_to_fixed_20(value: &str) -> Option<[u8; 20]> {
    let bytes = decode_hex_bytes(value)?;
    if bytes.len() != 20 {
        return None;
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Some(out)
}

fn decode_address_from_word(word: &str) -> Option<EvmAddress> {
    let bytes = decode_hex_bytes(word)?;
    decode_address_from_bytes(&bytes)
}

fn decode_address_from_bytes(bytes: &[u8]) -> Option<EvmAddress> {
    if bytes.len() < 32 {
        return None;
    }
    let tail = &bytes[bytes.len() - 20..];
    if tail.iter().all(|byte| *byte == 0) {
        return None;
    }
    EvmAddress::new(format!("0x{}", hex_lower(tail))).ok()
}

fn decode_string_output(value: &str) -> Option<String> {
    let bytes = decode_hex_bytes(value)?;
    if bytes.len() >= 64 {
        let offset = decode_word_usize(&bytes[0..32])?;
        if bytes.len() >= offset + 32 {
            let len = decode_word_usize(&bytes[offset..offset + 32])?;
            let start = offset + 32;
            let end = start.checked_add(len)?;
            if bytes.len() >= end {
                return String::from_utf8(bytes[start..end].to_vec()).ok();
            }
        }
    }
    if bytes.len() >= 32 {
        let end = bytes[..32].iter().position(|byte| *byte == 0).unwrap_or(32);
        if end == 0 {
            return None;
        }
        return String::from_utf8(bytes[..end].to_vec()).ok();
    }
    None
}

fn decode_word_usize(bytes: &[u8]) -> Option<usize> {
    decode_uint_u64(bytes).and_then(|value| usize::try_from(value).ok())
}

fn decode_uint_u64(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 32 {
        return None;
    }
    let word = &bytes[bytes.len() - 32..];
    if word[..24].iter().any(|byte| *byte != 0) {
        return None;
    }
    let mut out = 0u64;
    for byte in &word[24..] {
        out = (out << 8) | (*byte as u64);
    }
    Some(out)
}

fn decode_uint_decimal(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "0".to_string();
    }
    let word = if bytes.len() >= 32 {
        &bytes[bytes.len() - 32..]
    } else {
        bytes
    };
    unsigned_decimal_string(word)
}

fn decode_signed_decimal(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "0".to_string();
    }
    let word = if bytes.len() >= 32 {
        &bytes[bytes.len() - 32..]
    } else {
        bytes
    };
    if word[0] & 0x80 == 0 {
        return unsigned_decimal_string(word);
    }
    let mut magnitude = word.to_vec();
    for byte in &mut magnitude {
        *byte = !*byte;
    }
    for byte in magnitude.iter_mut().rev() {
        let (next, carry) = byte.overflowing_add(1);
        *byte = next;
        if !carry {
            break;
        }
    }
    format!("-{}", unsigned_decimal_string(&magnitude))
}

fn unsigned_decimal_string(bytes: &[u8]) -> String {
    let mut digits = vec![0u8];
    for byte in bytes {
        let mut carry = *byte as u32;
        for digit in &mut digits {
            let value = (*digit as u32) * 256 + carry;
            *digit = (value % 10) as u8;
            carry = value / 10;
        }
        while carry > 0 {
            digits.push((carry % 10) as u8);
            carry /= 10;
        }
    }
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    digits
        .iter()
        .rev()
        .map(|digit| char::from(b'0' + *digit))
        .collect()
}

fn decode_latest_round_data_response(
    response: Result<String, String>,
) -> Option<ChainlinkLatestRoundData> {
    let output = response.ok()?;
    let bytes = decode_hex_bytes(&output)?;
    if bytes.len() < 32 * 5 {
        return None;
    }
    Some(ChainlinkLatestRoundData {
        round_id: decode_uint_decimal(&bytes[0..32]),
        answer: decode_signed_decimal(&bytes[32..64]),
        started_at: decode_uint_u64(&bytes[64..96]).unwrap_or_default(),
        updated_at: decode_uint_u64(&bytes[96..128]).unwrap_or_default(),
        answered_in_round: decode_uint_decimal(&bytes[128..160]),
    })
}

fn decode_hex_bytes(value: &str) -> Option<Vec<u8>> {
    let body = value.trim().trim_start_matches("0x");
    if body.is_empty() {
        return Some(Vec::new());
    }
    if !body.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() / 2);
    let bytes = body.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let high = decode_hex_nibble(bytes[index])?;
        let low = decode_hex_nibble(bytes[index + 1])?;
        out.push((high << 4) | low);
        index += 2;
    }
    Some(out)
}

fn decode_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn parse_hex_u64(value: &str) -> Result<u64, String> {
    u64::from_str_radix(value.trim().trim_start_matches("0x"), 16)
        .map_err(|error| error.to_string())
}

fn is_non_positive_signed_decimal(value: &str) -> bool {
    value == "0" || value.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::discovery::{DependencyCandidateSource, DependencyDiscoveryReport};
    use crate::models::run::RunTarget;
    use crate::models::source::{DependencyFetchStatus, ProxyResolution, ProxyResolutionStatus};

    #[test]
    fn eip1967_slots_match_expected_constants() {
        assert_eq!(
            EIP1967_IMPLEMENTATION_SLOT,
            "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
        );
        assert_eq!(
            EIP1967_ADMIN_SLOT,
            "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103"
        );
        assert_eq!(
            EIP1967_BEACON_SLOT,
            "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50"
        );
    }

    #[test]
    fn decode_address_from_storage_word_extracts_low_twenty_bytes() {
        let decoded = decode_address_from_word(
            "0x00000000000000000000000052908400098527886e0f7030069857d2e4169ee7",
        )
        .expect("decoded address");
        assert_eq!(
            decoded.as_str(),
            "0x52908400098527886e0f7030069857d2e4169ee7"
        );
        assert!(
            decode_address_from_word(
                "0x0000000000000000000000000000000000000000000000000000000000000000"
            )
            .is_none()
        );
    }

    #[test]
    fn oracle_candidate_selection_uses_role_name_and_internal_type() {
        let bundle = SourceBundleArtifact {
            status: crate::models::step::StepStatus::SourceFetched,
            target: RunTarget::new(
                EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678").expect("address"),
                crate::models::identity::ChainAlias::new("eth").expect("chain"),
            ),
            dependency_discovery: Some(DependencyDiscoveryReport {
                merged_candidates: vec![
                    DependencyCandidate {
                        address: EvmAddress::new("0x52908400098527886E0F7030069857D2E4169EE7")
                            .expect("address"),
                        name: "priceFeed".into(),
                        role: "dependency".into(),
                        source: Some(DependencyCandidateSource::SourceConstant),
                        internal_type: "contract AggregatorV3Interface".into(),
                        declared_type: "AggregatorV3Interface".into(),
                        ..DependencyCandidate::default()
                    },
                    DependencyCandidate {
                        address: EvmAddress::new("0x8617E340B3D01FA5F11F306F4090FD50E238070D")
                            .expect("address"),
                        name: "router".into(),
                        role: "router".into(),
                        ..DependencyCandidate::default()
                    },
                ],
                ..DependencyDiscoveryReport::default()
            }),
            dependencies: vec![DependencyRecord {
                role: "oracle".into(),
                name: "sequencerOracle".into(),
                address: EvmAddress::new("0xde709f2102306220921060314715629080e2fb77")
                    .expect("address"),
                status: DependencyFetchStatus::Fetched,
                ..DependencyRecord::default()
            }],
            proxy_resolution: Some(ProxyResolution {
                status: ProxyResolutionStatus::ProviderFlagOnly,
                ..ProxyResolution::default()
            }),
            ..SourceBundleArtifact::default()
        };

        let oracles = collect_oracle_subjects(&bundle);
        let addresses = oracles
            .into_iter()
            .map(|item| item.address.as_lowercase())
            .collect::<BTreeSet<_>>();
        assert!(addresses.contains("0x52908400098527886e0f7030069857d2e4169ee7"));
        assert!(addresses.contains("0xde709f2102306220921060314715629080e2fb77"));
        assert!(!addresses.contains("0x8617e340b3d01fa5f11f306f4090fd50e238070d"));
    }

    #[test]
    fn proxy_signals_require_successful_impl_slot_read() {
        let result = ProxyCheckResult {
            address: "0x1234567890abcdef1234567890abcdef12345678".into(),
            provider_is_proxy: true,
            ..ProxyCheckResult::default()
        };

        let signals = proxy_signals(&result, false);

        assert!(
            !signals
                .iter()
                .any(|signal| signal.signal == "provider_proxy_without_eip1967_impl")
        );
    }

    #[test]
    fn proxy_signals_compare_provider_implementation_case_insensitively() {
        let result = ProxyCheckResult {
            address: "0x1234567890abcdef1234567890abcdef12345678".into(),
            provider_implementation: Some("0x52908400098527886E0F7030069857D2E4169EE7".into()),
            eip1967_implementation: Some("0x52908400098527886e0f7030069857d2e4169ee7".into()),
            ..ProxyCheckResult::default()
        };

        let signals = proxy_signals(&result, true);

        assert!(
            !signals
                .iter()
                .any(|signal| signal.signal == "eip1967_impl_differs_from_provider")
        );
    }

    #[test]
    fn oracle_signals_distinguish_failed_reads_from_no_standard_response() {
        let result = OracleCheckResult {
            address: "0x1234567890abcdef1234567890abcdef12345678".into(),
            failed_reads: vec![
                "decimals".into(),
                "description".into(),
                "version".into(),
                "latestRoundData".into(),
            ],
            evidence_artifacts: vec![WorkspaceRelPath::new(paths::ORACLE_CHECKS)],
            ..OracleCheckResult::default()
        };

        let signals = oracle_signals(&result, None);

        assert!(
            signals
                .iter()
                .any(|signal| signal.signal == "oracle_standard_read_failed")
        );
        assert!(
            !signals
                .iter()
                .any(|signal| signal.signal == "oracle_role_without_standard_interface_response")
        );
    }

    #[test]
    fn latest_round_data_decoder_handles_negative_answer_and_zero_timestamp() {
        let response = Ok(concat!(
            "0x",
            "0000000000000000000000000000000000000000000000000000000000000007",
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000007"
        )
        .to_string());

        let decoded = decode_latest_round_data_response(response).expect("decoded");
        assert_eq!(decoded.round_id, "7");
        assert_eq!(decoded.answer, "-1");
        assert_eq!(decoded.started_at, 1);
        assert_eq!(decoded.updated_at, 0);
        assert_eq!(decoded.answered_in_round, "7");
    }

    #[test]
    fn latest_round_data_decoder_returns_none_for_failed_call() {
        assert!(decode_latest_round_data_response(Err("reverted".into())).is_none());
    }
}
