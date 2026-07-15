use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use url::Url;

use crate::analysis::dependencies::analyze_dependencies;
use crate::analysis::discovery::discover_dependencies;
use crate::error::AppResult;
use crate::models::artifact::{ArtifactKind, ArtifactStatus, ArtifactStep};
use crate::models::bytecode::{BytecodeAuditTarget, BytecodeFetchStatus, BytecodeTargetsArtifact};
use crate::models::discovery::{DependencyCandidate, DependencyDiscoveryContext};
use crate::models::finding::{
    DependencyChainChecksArtifact, DependencyFindingsArtifact, FlashLoanSurfaceArtifact,
    OracleChecksArtifact, ProxyChecksArtifact,
};
use crate::models::identity::{ChainAlias, EvmAddress};
use crate::models::path::RelativePath;
use crate::models::path::WorkspaceRelPath;
use crate::models::run::RunTarget;
use crate::models::source::{
    AnalysisTarget, ArtifactSourceFile, ContractMetadata, DependencyFetchStatus, DependencyRecord,
    ProxyResolution, ProxyResolutionStatus, SourceAvailabilityStatus, SourceBundleArtifact,
    SourceFile, VerifiedSourceMetadata,
};
use crate::models::step::StepStatus;
use crate::services::source_provider::{
    SourceProviderFetch, fetch_verified_source, sanitize_dependency_name,
};
use crate::workspace::paths;

use super::AuditPipelineService;

const EIP1967_IMPLEMENTATION_SLOT: &str =
    "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc";
const EIP1967_ADMIN_SLOT: &str =
    "0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103";
const EIP1967_BEACON_SLOT: &str =
    "0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50";

struct BytecodeTargetCandidate {
    address: EvmAddress,
    chain: ChainAlias,
    role: String,
    name: String,
    source_availability: SourceAvailabilityStatus,
    source_unavailable_reason: Option<String>,
    origin: String,
    origin_evidence: Vec<WorkspaceRelPath>,
}

struct SourceRpcClient {
    url: Url,
    client: reqwest::blocking::Client,
}

#[derive(Default)]
struct Eip1967SlotProbe {
    implementation: Option<EvmAddress>,
}

impl AuditPipelineService {
    pub fn fetch_contract_source(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<StepStatus> {
        #[derive(Serialize)]
        struct SourceFetchRequestArtifactRef<'a> {
            address: &'a EvmAddress,
            chain: &'a ChainAlias,
            source_api_base: Option<&'a url::Url>,
            source_api_configured: bool,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            source_api_header_names: Vec<&'a str>,
            rpc_url_configured: bool,
        }
        let source_api_header_names = self
            .config
            .source_api_headers
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let request_path = self.workspace.store().write_json(
            paths::SOURCE_REQUEST,
            &SourceFetchRequestArtifactRef {
                address,
                chain,
                source_api_base: self.config.source_api_base.as_ref(),
                source_api_configured: self.config.source_api_base.is_some(),
                source_api_header_names,
                rpc_url_configured: self.config.rpc_url.is_some(),
            },
        )?;

        let Some(base_url) = self.config.source_api_base.as_ref() else {
            let bundle_path = self.workspace.store().write_json(
                paths::SOURCE_BUNDLE,
                &SourceBundleArtifact::not_configured(RunTarget::new(
                    address.clone(),
                    chain.clone(),
                )),
            )?;
            self.record(
                ArtifactStep::FetchContractSource,
                &request_path,
                ArtifactKind::Request,
                StepStatus::ConfiguredNotExecuted,
                "Persisted source fetch request metadata.",
            );
            self.record(
                ArtifactStep::FetchContractSource,
                &bundle_path,
                ArtifactKind::Artifact,
                StepStatus::ConfiguredNotExecuted,
                "Skipped source fetch because the source API is not configured.",
            );
            return Ok(StepStatus::SourceApiNotConfigured);
        };

        let source_fetch = match fetch_verified_source(
            base_url,
            self.config.source_api_key.as_deref(),
            &self.config.source_api_headers,
            address,
            chain,
        ) {
            Ok(bundle) => bundle,
            Err(error) => {
                let bundle_path = self.workspace.store().write_json(
                    paths::SOURCE_BUNDLE,
                    &SourceBundleArtifact::fetch_failed(
                        RunTarget::new(address.clone(), chain.clone()),
                        error.to_string(),
                        format!("{error:?}"),
                    ),
                )?;
                self.record(
                    ArtifactStep::FetchContractSource,
                    &request_path,
                    ArtifactKind::Request,
                    StepStatus::ExecutedWithError,
                    "Persisted source fetch request metadata.",
                );
                self.record(
                    ArtifactStep::FetchContractSource,
                    &bundle_path,
                    ArtifactKind::Artifact,
                    StepStatus::ExecutedWithError,
                    "Source fetch failed; inspect the stored error payload.",
                );
                return Ok(StepStatus::SourceFetchFailed);
            }
        };
        let bundle = match source_fetch {
            SourceProviderFetch::Verified(bundle) => bundle,
            SourceProviderFetch::SourceUnavailable(bundle) => {
                return self.handle_source_unavailable(
                    address,
                    chain,
                    request_path,
                    bundle.provider_payload,
                    bundle.normalized_payload,
                    bundle.reason,
                );
            }
        };

        let proxy_contract = &bundle.normalized_payload.contract;
        let provider_proxy = proxy_contract.proxy;
        let provider_implementation = proxy_contract.implementation.clone();
        let implementation_address = provider_implementation
            .as_ref()
            .filter(|implementation| *implementation != address);

        let raw_response_path = self
            .workspace
            .store()
            .write_json(paths::SOURCE_PROVIDER_RESPONSE, &bundle.provider_payload)?;
        let primary_sources =
            self.write_fetched_source_files(&bundle.files, None, "Stored a fetched source file.")?;

        let mut related_contracts = Vec::new();
        if provider_proxy && let Some(implementation_address) = implementation_address {
            related_contracts.push(self.fetch_dependency_bundle_record(
                implementation_address,
                chain,
                "implementation",
                "implementation",
                &RelativePath::new("implementation"),
            )?);
        }

        let source_map_for_discovery = self.source_map_for_discovery(&primary_sources)?;
        let dependency_discovery =
            discover_dependencies(&bundle.normalized_payload, &source_map_for_discovery);
        let dependencies = self.fetch_discovered_dependencies(
            &dependency_discovery.merged_candidates,
            address,
            chain,
            implementation_address.map_or_else(BTreeSet::new, |implementation| {
                BTreeSet::from([implementation.as_lowercase()])
            }),
        )?;

        let analysis_target = analysis_target_from_bundle(
            address,
            proxy_contract,
            &primary_sources,
            &related_contracts,
        );

        let mut bundle_payload =
            SourceBundleArtifact::from_verified_source(bundle.normalized_payload);
        bundle_payload.proxy_resolution = Some(ProxyResolution {
            status: ProxyResolutionStatus::ProviderFlagOnly,
            proxy: provider_proxy,
            implementation: provider_implementation,
        });
        bundle_payload.dependency_discovery = Some(dependency_discovery);
        bundle_payload.dependencies = dependencies;
        bundle_payload.related_contracts = related_contracts;
        bundle_payload.analysis_target = Some(analysis_target);

        let bundle_path = self
            .workspace
            .store()
            .write_json(paths::SOURCE_BUNDLE, &bundle_payload)?;
        let bytecode_targets = self.bytecode_candidates_from_bundle(&bundle_payload, chain);
        let (bytecode_targets_path, bytecode_targets_status) = self.write_bytecode_targets(
            &RunTarget::new(address.clone(), chain.clone()),
            bytecode_targets,
        )?;

        self.record(
            ArtifactStep::FetchContractSource,
            &request_path,
            ArtifactKind::Request,
            ArtifactStatus::Executed,
            "Persisted source fetch request metadata.",
        );
        self.record(
            ArtifactStep::FetchContractSource,
            &raw_response_path,
            ArtifactKind::Artifact,
            ArtifactStatus::Executed,
            "Stored the raw source provider response.",
        );
        self.record(
            ArtifactStep::FetchContractSource,
            &bundle_path,
            ArtifactKind::Artifact,
            ArtifactStatus::Executed,
            "Fetched and normalized verified source metadata.",
        );
        self.record(
            ArtifactStep::FetchContractSource,
            &bytecode_targets_path,
            ArtifactKind::Artifact,
            bytecode_targets_status,
            "Stored bytecode audit target metadata for source-unavailable contracts.",
        );
        Ok(StepStatus::SourceFetched)
    }

    fn handle_source_unavailable(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        request_path: WorkspaceRelPath,
        provider_payload: Value,
        metadata: VerifiedSourceMetadata,
        reason: String,
    ) -> AppResult<StepStatus> {
        let raw_response_path = self
            .workspace
            .store()
            .write_json(paths::SOURCE_PROVIDER_RESPONSE, &provider_payload)?;

        let provider_contract = metadata.contract.clone();
        let provider_implementation = provider_contract
            .implementation
            .as_ref()
            .filter(|implementation| *implementation != address)
            .cloned();
        let slot_probe = self.eip1967_slot_probe(address);
        let slot_implementation = slot_probe
            .as_ref()
            .and_then(|probe| probe.implementation.as_ref())
            .filter(|implementation| *implementation != address)
            .cloned();

        let mut related_contracts = Vec::new();
        let mut seen_implementations = BTreeSet::new();
        for (implementation, origin) in [
            (provider_implementation.as_ref(), "provider"),
            (slot_implementation.as_ref(), "eip1967"),
        ] {
            let Some(implementation) = implementation else {
                continue;
            };
            if !seen_implementations.insert(implementation.as_lowercase()) {
                continue;
            }
            let prefix = RelativePath::new(format!(
                "implementation/{origin}_{}",
                implementation.as_lowercase()
            ));
            related_contracts.push(self.fetch_dependency_bundle_record(
                implementation,
                chain,
                "implementation",
                "implementation",
                &prefix,
            )?);
        }

        let mut bundle_payload = SourceBundleArtifact::source_unavailable(metadata, reason);
        let resolved_implementation = provider_implementation.or(slot_implementation);
        bundle_payload.proxy_resolution = Some(ProxyResolution {
            status: if slot_probe.is_some() {
                ProxyResolutionStatus::Eip1967Slots
            } else {
                ProxyResolutionStatus::ProviderFlagOnly
            },
            proxy: provider_contract.proxy || resolved_implementation.is_some(),
            implementation: resolved_implementation,
        });
        bundle_payload.related_contracts = related_contracts;
        bundle_payload.analysis_target = Some(AnalysisTarget {
            address: address.clone(),
            contract_name: provider_contract.name,
            role: "target".to_string(),
            ..AnalysisTarget::default()
        });

        let bundle_path = self
            .workspace
            .store()
            .write_json(paths::SOURCE_BUNDLE, &bundle_payload)?;
        let bytecode_targets = self.bytecode_candidates_from_bundle(&bundle_payload, chain);
        let (bytecode_targets_path, bytecode_targets_status) = self.write_bytecode_targets(
            &RunTarget::new(address.clone(), chain.clone()),
            bytecode_targets,
        )?;

        self.record(
            ArtifactStep::FetchContractSource,
            &request_path,
            ArtifactKind::Request,
            ArtifactStatus::Executed,
            "Persisted source fetch request metadata.",
        );
        self.record(
            ArtifactStep::FetchContractSource,
            &raw_response_path,
            ArtifactKind::Artifact,
            ArtifactStatus::Executed,
            "Stored the raw source provider response.",
        );
        self.record(
            ArtifactStep::FetchContractSource,
            &bundle_path,
            ArtifactKind::Artifact,
            ArtifactStatus::Executed,
            "Stored source-unavailable metadata for bytecode-oriented review.",
        );
        self.record(
            ArtifactStep::FetchContractSource,
            &bytecode_targets_path,
            ArtifactKind::Artifact,
            bytecode_targets_status,
            "Stored bytecode audit target metadata for source-unavailable contracts.",
        );
        Ok(StepStatus::SourceUnavailable)
    }

    pub fn run_dependency_analysis(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<StepStatus> {
        let bundle_payload = self.load_source_bundle_payload()?;
        let target = RunTarget::new(address.clone(), chain.clone());
        if !bundle_payload.is_fetched() && !bundle_payload.is_source_unavailable() {
            let chain_artifacts = self.build_dependency_chain_artifacts(&bundle_payload, &target);
            let findings_path = self.workspace.store().write_json(
                paths::DEPENDENCY_FINDINGS,
                &DependencyFindingsArtifact::new(target, StepStatus::SourceNotFetched, Vec::new()),
            )?;
            self.record(
                ArtifactStep::RunDependencyAnalysis,
                &findings_path,
                ArtifactKind::Artifact,
                StepStatus::ConfiguredNotExecuted,
                "Skipped dependency analysis because source fetching did not complete.",
            );
            self.write_dependency_chain_artifacts(chain_artifacts)?;
            return Ok(StepStatus::SourceNotFetched);
        }

        let findings = if bundle_payload.is_fetched() {
            analyze_dependencies(&bundle_payload, self.workspace.root())
        } else {
            Vec::new()
        };
        let chain_artifacts = self.build_dependency_chain_artifacts(&bundle_payload, &target);
        let status = if bundle_payload.is_fetched() {
            StepStatus::Executed
        } else {
            StepStatus::SourceUnavailable
        };
        let findings_path = self.workspace.store().write_json(
            paths::DEPENDENCY_FINDINGS,
            &DependencyFindingsArtifact::new(target, status, findings),
        )?;
        self.record(
            ArtifactStep::RunDependencyAnalysis,
            &findings_path,
            ArtifactKind::Artifact,
            status,
            "Analyzed fetched dependencies for high-signal role-specific findings.",
        );
        self.write_dependency_chain_artifacts(chain_artifacts)?;
        Ok(status)
    }

    fn write_dependency_chain_artifacts(
        &mut self,
        artifacts: super::dependency_chain::DependencyChainArtifacts,
    ) -> AppResult<()> {
        let super::dependency_chain::DependencyChainArtifacts {
            summary,
            proxy,
            oracle,
            flash,
        } = artifacts;
        self.write_dependency_chain_summary(&summary)?;
        self.write_proxy_checks(&proxy)?;
        self.write_oracle_checks(&oracle)?;
        self.write_flash_loan_surface(&flash)?;
        Ok(())
    }

    fn write_dependency_chain_summary(
        &mut self,
        payload: &DependencyChainChecksArtifact,
    ) -> AppResult<()> {
        self.write_dependency_chain_payload(
            paths::DEPENDENCY_CHAIN_CHECKS,
            payload,
            payload.status.artifact_status(),
            "Stored non-mutating dependency chain-check summary.",
        )
    }

    fn write_proxy_checks(&mut self, payload: &ProxyChecksArtifact) -> AppResult<()> {
        self.write_dependency_chain_payload(
            paths::PROXY_CHECKS,
            payload,
            payload.status.artifact_status(),
            "Stored proxy upgradeability review signals for the target and dependencies.",
        )
    }

    fn write_oracle_checks(&mut self, payload: &OracleChecksArtifact) -> AppResult<()> {
        self.write_dependency_chain_payload(
            paths::ORACLE_CHECKS,
            payload,
            payload.status.artifact_status(),
            "Stored oracle configuration and liveness checks for candidate dependencies.",
        )
    }

    fn write_flash_loan_surface(&mut self, payload: &FlashLoanSurfaceArtifact) -> AppResult<()> {
        self.write_dependency_chain_payload(
            paths::FLASH_LOAN_SURFACE,
            payload,
            payload.status.artifact_status(),
            "Stored dependency surface mapping relevant to flash-loan-style simulations.",
        )
    }

    fn write_dependency_chain_payload<T: serde::Serialize>(
        &mut self,
        relative_path: &str,
        payload: &T,
        status: StepStatus,
        summary: &str,
    ) -> AppResult<()> {
        let path = self.workspace.store().write_json(relative_path, payload)?;
        self.record(
            ArtifactStep::RunDependencyAnalysis,
            &path,
            ArtifactKind::Artifact,
            status,
            summary,
        );
        Ok(())
    }

    fn bytecode_candidates_from_bundle(
        &self,
        bundle: &SourceBundleArtifact,
        chain: &ChainAlias,
    ) -> Vec<BytecodeTargetCandidate> {
        let mut candidates = Vec::new();
        let mut seen = BTreeSet::new();
        if bundle.is_source_unavailable() {
            seen.insert(bundle.target.address.as_lowercase());
            candidates.push(BytecodeTargetCandidate {
                address: bundle.target.address.clone(),
                chain: chain.clone(),
                role: "target".to_string(),
                name: bundle
                    .contract
                    .as_ref()
                    .map(|contract| contract.name.clone())
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "target".to_string()),
                source_availability: bundle.source_availability,
                source_unavailable_reason: bundle.source_unavailable_reason.clone(),
                origin: "source_provider_unavailable".to_string(),
                origin_evidence: vec![
                    WorkspaceRelPath::new(paths::SOURCE_BUNDLE),
                    WorkspaceRelPath::new(paths::SOURCE_PROVIDER_RESPONSE),
                ],
            });
        }
        for record in bundle
            .related_contracts
            .iter()
            .chain(bundle.dependencies.iter())
        {
            collect_bytecode_candidates_from_record(record, chain, &mut seen, &mut candidates);
        }
        candidates
    }

    fn write_bytecode_targets(
        &mut self,
        target: &RunTarget,
        candidates: Vec<BytecodeTargetCandidate>,
    ) -> AppResult<(WorkspaceRelPath, StepStatus)> {
        let rpc_url_configured = self.config.rpc_url.is_some();
        let rpc_client = self
            .config
            .rpc_url
            .as_ref()
            .and_then(|url| SourceRpcClient::new(url).ok());
        let rpc_init_error = if rpc_url_configured && rpc_client.is_none() {
            Some("failed to initialize RPC client".to_string())
        } else {
            None
        };
        let mut targets = Vec::new();
        for candidate in candidates {
            let mut item = BytecodeAuditTarget {
                address: candidate.address.clone(),
                chain: candidate.chain.clone(),
                role: candidate.role,
                name: candidate.name,
                source_availability: candidate.source_availability,
                source_unavailable_reason: candidate.source_unavailable_reason,
                origin: candidate.origin,
                origin_evidence: candidate.origin_evidence,
                ..BytecodeAuditTarget::default()
            };
            match rpc_client.as_ref() {
                None if !rpc_url_configured => {
                    item.bytecode_status = BytecodeFetchStatus::RpcNotConfigured;
                    item.error = Some("AGENT_AUDIT_RPC_URL is not configured".to_string());
                }
                None => {
                    item.bytecode_status = BytecodeFetchStatus::FetchFailed;
                    item.error = rpc_init_error.clone();
                }
                Some(client) => match client.get_code(&candidate.address) {
                    Ok(bytecode) if is_empty_bytecode(&bytecode) => {
                        item.bytecode_status = BytecodeFetchStatus::EmptyCode;
                        item.error =
                            Some("eth_getCode returned empty runtime bytecode".to_string());
                    }
                    Ok(bytecode) => {
                        let artifact_path =
                            bytecode_artifact_path(&candidate.chain, &candidate.address);
                        self.workspace
                            .store()
                            .write_text(artifact_path.as_str(), &format!("{bytecode}\n"))?;
                        self.record(
                            ArtifactStep::FetchContractSource,
                            &artifact_path,
                            ArtifactKind::Artifact,
                            ArtifactStatus::Executed,
                            "Stored runtime bytecode for a source-unavailable audit target.",
                        );
                        item.bytecode_status = BytecodeFetchStatus::Fetched;
                        item.bytecode_artifact = Some(artifact_path);
                    }
                    Err(error) => {
                        item.bytecode_status = BytecodeFetchStatus::FetchFailed;
                        item.error = Some(error);
                    }
                },
            }
            targets.push(item);
        }
        let artifact = BytecodeTargetsArtifact::new(target.clone(), rpc_url_configured, targets);
        let status = artifact.status;
        let path = self
            .workspace
            .store()
            .write_json(paths::BYTECODE_TARGETS, &artifact)?;
        Ok((path, status))
    }

    fn eip1967_slot_probe(&self, address: &EvmAddress) -> Option<Eip1967SlotProbe> {
        let client = self
            .config
            .rpc_url
            .as_ref()
            .and_then(|url| SourceRpcClient::new(url).ok())?;
        let implementation = client
            .get_storage_at(address, EIP1967_IMPLEMENTATION_SLOT)
            .ok()
            .and_then(|word| decode_address_from_word(&word));
        let _admin = client.get_storage_at(address, EIP1967_ADMIN_SLOT);
        let _beacon = client.get_storage_at(address, EIP1967_BEACON_SLOT);
        Some(Eip1967SlotProbe { implementation })
    }

    fn source_map_for_discovery(
        &self,
        primary_sources: &[ArtifactSourceFile],
    ) -> AppResult<BTreeMap<String, String>> {
        let mut source_map_for_discovery = BTreeMap::new();
        for item in primary_sources {
            let relative_path = item.path.as_str();
            let file_path = self.workspace.root().join("sources").join(relative_path);
            if file_path.exists() {
                source_map_for_discovery
                    .insert(relative_path.to_string(), fs::read_to_string(file_path)?);
            }
        }
        Ok(source_map_for_discovery)
    }

    fn write_fetched_source_files(
        &mut self,
        files: &[SourceFile],
        prefix: Option<&RelativePath>,
        summary_prefix: &str,
    ) -> AppResult<Vec<ArtifactSourceFile>> {
        let mut written = Vec::new();
        for source_file in files {
            let final_path = if let Some(prefix) = prefix {
                prefix.join(source_file.path.as_str())
            } else {
                source_file.path.clone()
            };
            self.write_source_text(source_file, &final_path, summary_prefix)?;
            written.push(ArtifactSourceFile {
                path: final_path,
                length: source_file.content.len(),
                original_path: prefix.map(|_| source_file.path.clone()),
            });
        }
        Ok(written)
    }

    fn fetch_dependency_bundle_record(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
        role: &str,
        name: &str,
        prefix: &RelativePath,
    ) -> AppResult<DependencyRecord> {
        let Some(base_url) = self.config.source_api_base.as_ref() else {
            return Ok(DependencyRecord {
                role: role.to_string(),
                name: name.to_string(),
                address: address.clone(),
                status: DependencyFetchStatus::FetchFailed,
                source_availability: SourceAvailabilityStatus::Unknown,
                error: Some("missing source API base".to_string()),
                ..DependencyRecord::default()
            });
        };

        let source_fetch = match fetch_verified_source(
            base_url,
            self.config.source_api_key.as_deref(),
            &self.config.source_api_headers,
            address,
            chain,
        ) {
            Ok(bundle) => bundle,
            Err(error) => {
                return Ok(DependencyRecord {
                    role: role.to_string(),
                    name: name.to_string(),
                    address: address.clone(),
                    status: DependencyFetchStatus::FetchFailed,
                    source_availability: SourceAvailabilityStatus::Unknown,
                    error: Some(error.to_string()),
                    ..DependencyRecord::default()
                });
            }
        };
        let bundle = match source_fetch {
            SourceProviderFetch::Verified(bundle) => bundle,
            SourceProviderFetch::SourceUnavailable(bundle) => {
                let response_artifact = self.workspace.store().write_json(
                    format!(
                        "artifacts/source_provider_response_{}.json",
                        prefix.as_str().replace('/', "_")
                    ),
                    &bundle.provider_payload,
                )?;
                self.record(
                    ArtifactStep::FetchContractSource,
                    &response_artifact,
                    ArtifactKind::Artifact,
                    ArtifactStatus::Executed,
                    "Stored the raw dependency provider response.",
                );
                let VerifiedSourceMetadata {
                    provider,
                    contract,
                    compiler,
                    abi,
                    source_layout,
                    source_meta,
                    ..
                } = bundle.normalized_payload;
                return Ok(DependencyRecord {
                    role: role.to_string(),
                    name: name.to_string(),
                    address: address.clone(),
                    source_availability: SourceAvailabilityStatus::Unavailable,
                    source_unavailable_reason: Some(bundle.reason),
                    provider: Some(provider),
                    contract: Some(contract),
                    compiler: Some(compiler),
                    abi,
                    source_layout,
                    source_meta: Some(source_meta),
                    provider_response_artifact: Some(response_artifact),
                    status: DependencyFetchStatus::SourceUnavailable,
                    error: Some("verified source is unavailable for this address".to_string()),
                    ..DependencyRecord::default()
                });
            }
        };

        let response_artifact = self.workspace.store().write_json(
            format!(
                "artifacts/source_provider_response_{}.json",
                prefix.as_str().replace('/', "_")
            ),
            &bundle.provider_payload,
        )?;
        self.record(
            ArtifactStep::FetchContractSource,
            &response_artifact,
            ArtifactKind::Artifact,
            ArtifactStatus::Executed,
            "Stored the raw dependency provider response.",
        );
        let written_files = self.write_fetched_source_files(
            &bundle.files,
            Some(prefix),
            "Stored a fetched dependency source file.",
        )?;
        let VerifiedSourceMetadata {
            provider,
            contract,
            compiler,
            abi,
            source_layout,
            source_meta,
            ..
        } = bundle.normalized_payload;

        let mut record = DependencyRecord {
            role: role.to_string(),
            name: name.to_string(),
            address: address.clone(),
            provider: Some(provider),
            contract: Some(contract),
            compiler: Some(compiler),
            abi,
            source_layout,
            source_meta: Some(source_meta),
            files: written_files,
            provider_response_artifact: Some(response_artifact),
            status: DependencyFetchStatus::Fetched,
            source_availability: SourceAvailabilityStatus::Verified,
            related_contracts: Vec::new(),
            ..DependencyRecord::default()
        };
        if record
            .contract
            .as_ref()
            .is_some_and(|contract| contract.proxy)
            && let Some(implementation_address) = record
                .contract
                .as_ref()
                .and_then(|contract| contract.implementation.as_ref())
                .filter(|implementation| *implementation != address)
        {
            let nested = self.fetch_dependency_bundle_record(
                implementation_address,
                chain,
                "implementation",
                &format!("{name}-implementation"),
                &prefix.join("implementation"),
            )?;
            record.related_contracts.push(nested);
        }
        Ok(record)
    }

    fn fetch_discovered_dependencies(
        &mut self,
        candidates: &[DependencyCandidate],
        target_address: &EvmAddress,
        chain: &ChainAlias,
        skip_addresses: BTreeSet<String>,
    ) -> AppResult<Vec<DependencyRecord>> {
        let mut records = Vec::new();
        let mut seen = BTreeSet::new();
        seen.insert(target_address.as_lowercase());
        seen.extend(skip_addresses);
        for item in candidates {
            let address = &item.address;
            let address_key = address.as_lowercase();
            if seen.contains(address_key.as_str()) {
                continue;
            }
            let role = if item.role.is_empty() {
                "dependency"
            } else {
                item.role.as_str()
            };
            let name = if item.name.is_empty() {
                role
            } else {
                item.name.as_str()
            };
            let safe_name = sanitize_dependency_name(name);
            let prefix =
                RelativePath::new(format!("dependencies/{role}/{safe_name}_{address_key}"));
            seen.insert(address_key);
            let mut record =
                self.fetch_dependency_bundle_record(address, chain, role, name, &prefix)?;
            record.discovery = Some(DependencyDiscoveryContext {
                sources: item.sources.clone(),
                internal_type: item.internal_type.clone(),
                solidity_type: item.solidity_type.clone(),
                file: item.file.clone(),
            });
            records.push(record);
        }
        Ok(records)
    }
}

fn collect_bytecode_candidates_from_record(
    record: &DependencyRecord,
    chain: &ChainAlias,
    seen: &mut BTreeSet<String>,
    candidates: &mut Vec<BytecodeTargetCandidate>,
) {
    if record.is_source_unavailable() && seen.insert(record.address.as_lowercase()) {
        let mut evidence = vec![WorkspaceRelPath::new(paths::SOURCE_BUNDLE)];
        if let Some(path) = record.provider_response_artifact.as_ref() {
            evidence.push(path.clone());
        }
        candidates.push(BytecodeTargetCandidate {
            address: record.address.clone(),
            chain: chain.clone(),
            role: record.role.clone(),
            name: if record.name.is_empty() {
                record.role.clone()
            } else {
                record.name.clone()
            },
            source_availability: record.source_availability,
            source_unavailable_reason: record.source_unavailable_reason.clone(),
            origin: format!("{}_source_unavailable", record.role),
            origin_evidence: evidence,
        });
    }
    for nested in &record.related_contracts {
        collect_bytecode_candidates_from_record(nested, chain, seen, candidates);
    }
}

fn bytecode_artifact_path(chain: &ChainAlias, address: &EvmAddress) -> WorkspaceRelPath {
    WorkspaceRelPath::new(format!(
        "artifacts/bytecode/{}/{}.hex",
        chain.as_str(),
        address.as_lowercase()
    ))
}

fn is_empty_bytecode(bytecode: &str) -> bool {
    let body = bytecode.trim().trim_start_matches("0x");
    body.is_empty() || body.chars().all(|ch| ch == '0')
}

impl SourceRpcClient {
    fn new(url: &Url) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            url: url.clone(),
            client,
        })
    }

    fn get_code(&self, address: &EvmAddress) -> Result<String, String> {
        self.request("eth_getCode", json!([address.as_str(), "latest"]))
            .and_then(|value| {
                value
                    .as_str()
                    .map(normalize_hex_string)
                    .ok_or_else(|| "eth_getCode returned a non-string result".to_string())
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
                .map(normalize_hex_string)
                .ok_or_else(|| "eth_getStorageAt returned a non-string result".to_string())
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

fn normalize_hex_string(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with("0x") {
        trimmed.to_ascii_lowercase()
    } else {
        format!("0x{}", trimmed.to_ascii_lowercase())
    }
}

fn decode_address_from_word(word: &str) -> Option<EvmAddress> {
    let bytes = decode_hex_bytes(word)?;
    if bytes.len() < 32 {
        return None;
    }
    let tail = &bytes[bytes.len() - 20..];
    if tail.iter().all(|byte| *byte == 0) {
        return None;
    }
    EvmAddress::new(format!("0x{}", hex_lower(tail))).ok()
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

pub(super) fn analysis_target_from_bundle(
    address: &EvmAddress,
    primary_contract: &ContractMetadata,
    primary_files: &[ArtifactSourceFile],
    related_contracts: &[DependencyRecord],
) -> AnalysisTarget {
    for related in related_contracts {
        if related.role == "implementation"
            && related.is_fetched()
            && let Some(first_path) = related.files.first().map(|item| item.path.clone())
        {
            return AnalysisTarget {
                address: related.address.clone(),
                contract_name: related
                    .contract
                    .as_ref()
                    .map(|contract| contract.name.clone())
                    .unwrap_or_default(),
                path: first_path,
                role: "implementation".to_string(),
                ..AnalysisTarget::default()
            };
        }
    }

    let first_primary_path = if let Some(preferred_path) = primary_contract
        .file_name
        .as_ref()
        .filter(|preferred| primary_files.iter().any(|item| item.path == **preferred))
    {
        preferred_path.clone()
    } else {
        primary_files
            .first()
            .map(|item| item.path.clone())
            .unwrap_or_default()
    };
    AnalysisTarget {
        address: address.clone(),
        contract_name: primary_contract.name.clone(),
        path: first_primary_path,
        role: "target".to_string(),
        ..AnalysisTarget::default()
    }
}

pub(super) fn analysis_target_for_prepared(bundle: &SourceBundleArtifact) -> AnalysisTarget {
    if let Some(preferred_path) = bundle
        .contract
        .as_ref()
        .and_then(|contract| contract.file_name.as_ref())
        .filter(|preferred| record_for_path(bundle, preferred).is_some())
    {
        let prepared_path = preferred_path.clone();
        return AnalysisTarget {
            address: bundle.target.address.clone(),
            contract_name: bundle
                .contract
                .as_ref()
                .map(|contract| contract.name.clone())
                .unwrap_or_default(),
            path: prepared_path.clone(),
            role: "target".to_string(),
            prepared_path: Some(prepared_path),
            ..AnalysisTarget::default()
        };
    }

    if let Some(analysis_target) = bundle.analysis_target.as_ref() {
        let prepared_path = analysis_target.path.clone();
        return AnalysisTarget {
            address: analysis_target.address.clone(),
            contract_name: analysis_target.contract_name.clone(),
            path: prepared_path.clone(),
            role: analysis_target.role.clone(),
            prepared_path: Some(prepared_path),
            ..AnalysisTarget::default()
        };
    }

    let prepared_path = bundle
        .files
        .first()
        .map(|item| item.path.clone())
        .unwrap_or_default();
    AnalysisTarget {
        address: bundle.target.address.clone(),
        contract_name: bundle
            .contract
            .as_ref()
            .map(|contract| contract.name.clone())
            .unwrap_or_default(),
        path: prepared_path.clone(),
        role: "target".to_string(),
        prepared_path: Some(prepared_path),
        ..AnalysisTarget::default()
    }
}

fn collect_bundle_records(bundle: &SourceBundleArtifact) -> Vec<BundleRecordRef<'_>> {
    let mut records = vec![BundleRecordRef::Target(bundle)];
    for record in &bundle.dependencies {
        records.extend(collect_record_tree(record));
    }
    for record in &bundle.related_contracts {
        records.extend(collect_record_tree(record));
    }
    records
}

fn collect_record_tree(record: &DependencyRecord) -> Vec<BundleRecordRef<'_>> {
    let mut records = vec![BundleRecordRef::Dependency(record)];
    for nested in &record.related_contracts {
        records.extend(collect_record_tree(nested));
    }
    records
}

fn record_for_path<'a>(
    bundle: &'a SourceBundleArtifact,
    relative_path: &RelativePath,
) -> Option<BundleRecordRef<'a>> {
    collect_bundle_records(bundle).into_iter().find(|record| {
        record
            .files()
            .iter()
            .any(|item| item.path == *relative_path)
    })
}

pub(super) fn compiler_version_for_path(
    bundle: &SourceBundleArtifact,
    relative_path: &RelativePath,
) -> String {
    record_for_path(bundle, relative_path)
        .map(|record| record.compiler_version().to_string())
        .unwrap_or_default()
}

pub(super) fn source_meta_for_path<'a>(
    bundle: &'a SourceBundleArtifact,
    relative_path: &RelativePath,
) -> Option<&'a crate::models::source::SourceMetadata> {
    match record_for_path(bundle, relative_path) {
        Some(BundleRecordRef::Target(bundle)) => bundle.source_meta.as_ref(),
        Some(BundleRecordRef::Dependency(record)) => record.source_meta.as_ref(),
        None => None,
    }
}

enum BundleRecordRef<'a> {
    Target(&'a SourceBundleArtifact),
    Dependency(&'a DependencyRecord),
}

impl<'a> BundleRecordRef<'a> {
    fn files(&self) -> &[ArtifactSourceFile] {
        match self {
            Self::Target(bundle) => &bundle.files,
            Self::Dependency(record) => &record.files,
        }
    }

    fn compiler_version(&self) -> &str {
        match self {
            Self::Target(bundle) => bundle
                .compiler
                .as_ref()
                .map(|compiler| compiler.version.as_str())
                .unwrap_or_default(),
            Self::Dependency(record) => record
                .compiler
                .as_ref()
                .map(|compiler| compiler.version.as_str())
                .unwrap_or_default(),
        }
    }
}

#[allow(dead_code)]
fn _metadata_ref(_metadata: &VerifiedSourceMetadata) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::models::bytecode::{BytecodeFetchStatus, BytecodeTargetsArtifact};
    use crate::models::finding::{
        ChainCheckStatus, DependencyChainChecksArtifact, DependencyFindingsArtifact,
        FlashLoanSurfaceArtifact, OracleChecksArtifact, ProxyChecksArtifact,
    };
    use crate::models::identity::{ChainAlias, EvmAddress, RunId};
    use crate::models::run::{RunRequest, RunTarget};
    use crate::workspace::RunWorkspace;
    use tempfile::TempDir;
    use url::Url;

    fn test_workspace() -> (TempDir, RunWorkspace, RunTarget) {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(
            temp.path().join(".env"),
            "AGENT_AUDIT_DEFAULT_CHAIN=eth\nAGENT_AUDIT_RUNS_DIR=runs\n",
        )
        .expect("write env");
        let target = RunTarget::new(
            EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678").expect("address"),
            ChainAlias::new("eth").expect("chain"),
        );
        let workspace = RunWorkspace::create_at_root(
            temp.path(),
            &temp.path().join("runs/run-1"),
            &RunId::new("run-1").expect("run id"),
            &target.address,
            &target.chain,
        )
        .expect("workspace");
        workspace
            .store()
            .write_json(
                paths::REQUEST,
                &RunRequest {
                    address: target.address.clone(),
                    chain: target.chain.clone(),
                },
            )
            .expect("write request");
        (temp, workspace, target)
    }

    fn test_config(workspace: &RunWorkspace, target: &RunTarget) -> AppConfig {
        AppConfig {
            project_root: workspace.project_root.clone(),
            runs_dir: workspace.project_root.join("runs"),
            default_chain: target.chain.clone(),
            source_api_base: None,
            source_api_key: None,
            source_api_headers: BTreeMap::new(),
            rpc_url: None,
            mongo_uri: None,
            mongo_db: "agent_audit".to_string(),
            mongo_runs_meta_collection: "runs_meta".to_string(),
            mongo_runs_files_collection: "runs_files".to_string(),
            mongo_max_inline_file_bytes: 8 * 1024 * 1024,
        }
    }

    fn spawn_mock_rpc_server(implementation: Option<&EvmAddress>) -> String {
        let implementation_word = implementation.map(eip1967_address_word);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock rpc");
        let address = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            for stream in listener.incoming().take(16) {
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(_) => continue,
                };
                let mut buffer = [0u8; 8192];
                let bytes = match std::io::Read::read(&mut stream, &mut buffer) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                let request = String::from_utf8_lossy(&buffer[..bytes]);
                let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
                let payload: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                let method = payload
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let result = match method {
                    "eth_getCode" => Value::String("0x60016002".to_string()),
                    "eth_getStorageAt" => {
                        let slot = payload
                            .get("params")
                            .and_then(Value::as_array)
                            .and_then(|params| params.get(1))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if slot == EIP1967_IMPLEMENTATION_SLOT {
                            implementation_word
                                .clone()
                                .map(Value::String)
                                .unwrap_or_else(zero_word)
                        } else {
                            zero_word()
                        }
                    }
                    _ => Value::Null,
                };
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": payload.get("id").cloned().unwrap_or(Value::from(1)),
                    "result": result
                })
                .to_string();
                let http = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
                let _ = std::io::Write::write_all(&mut stream, http.as_bytes());
                let _ = std::io::Write::flush(&mut stream);
            }
        });
        format!("http://{address}")
    }

    fn spawn_source_unavailable_api_server() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind source api");
        let address = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            for stream in listener.incoming().take(8) {
                let mut stream = match stream {
                    Ok(stream) => stream,
                    Err(_) => continue,
                };
                let mut buffer = [0u8; 4096];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let response = json!({
                    "status": "1",
                    "message": "OK",
                    "result": [{
                        "SourceCode": "",
                        "ABI": "Contract source code not verified",
                        "ContractName": "UnverifiedContract",
                        "ContractFileName": "",
                        "CompilerVersion": "",
                        "CompilerType": "",
                        "OptimizationUsed": "",
                        "Runs": "",
                        "EVMVersion": "",
                        "ConstructorArguments": "",
                        "LicenseType": "",
                        "Library": "",
                        "SwarmSource": "",
                        "Proxy": "0",
                        "Implementation": "",
                        "SimilarMatch": ""
                    }]
                })
                .to_string();
                let http = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    response.len(),
                    response
                );
                let _ = std::io::Write::write_all(&mut stream, http.as_bytes());
                let _ = std::io::Write::flush(&mut stream);
            }
        });
        format!("http://{address}")
    }

    fn zero_word() -> Value {
        Value::String(
            "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        )
    }

    fn eip1967_address_word(address: &EvmAddress) -> String {
        format!(
            "0x000000000000000000000000{}",
            address.as_lowercase().trim_start_matches("0x")
        )
    }

    #[test]
    fn missing_rpc_writes_skipped_chain_artifacts_and_preserves_dependency_findings() {
        let (_temp, workspace, target) = test_workspace();
        workspace
            .store()
            .write_json(
                paths::SOURCE_BUNDLE,
                &SourceBundleArtifact {
                    target: target.clone(),
                    status: StepStatus::SourceFetched,
                    contract: Some(ContractMetadata {
                        name: "ProxyTarget".into(),
                        proxy: true,
                        ..ContractMetadata::default()
                    }),
                    ..SourceBundleArtifact::default()
                },
            )
            .expect("source bundle");
        let config = test_config(&workspace, &target);
        let mut service = AuditPipelineService::new(config, workspace);

        let status = service
            .run_dependency_analysis(&target.address, &target.chain)
            .expect("run dependency");
        assert_eq!(status, StepStatus::Executed);

        let findings: DependencyFindingsArtifact = super::super::support::read_json_if_exists(
            &service
                .workspace
                .paths()
                .resolve(paths::DEPENDENCY_FINDINGS),
        )
        .expect("findings");
        assert_eq!(findings.status, StepStatus::Executed);

        let summary: DependencyChainChecksArtifact = super::super::support::read_json_if_exists(
            &service
                .workspace
                .paths()
                .resolve(paths::DEPENDENCY_CHAIN_CHECKS),
        )
        .expect("summary");
        assert_eq!(summary.status, ChainCheckStatus::RpcNotConfigured);
        assert!(summary.summary_signals.is_empty());

        let proxy: ProxyChecksArtifact = super::super::support::read_json_if_exists(
            &service.workspace.paths().resolve(paths::PROXY_CHECKS),
        )
        .expect("proxy");
        assert_eq!(proxy.status, ChainCheckStatus::RpcNotConfigured);
        assert!(proxy.checks.iter().all(|check| check.signals.is_empty()));

        let oracle: OracleChecksArtifact = super::super::support::read_json_if_exists(
            &service.workspace.paths().resolve(paths::ORACLE_CHECKS),
        )
        .expect("oracle");
        assert_eq!(oracle.status, ChainCheckStatus::RpcNotConfigured);

        let flash: FlashLoanSurfaceArtifact = super::super::support::read_json_if_exists(
            &service.workspace.paths().resolve(paths::FLASH_LOAN_SURFACE),
        )
        .expect("flash");
        assert_eq!(flash.status, ChainCheckStatus::RpcNotConfigured);
    }

    #[test]
    fn source_not_fetched_writes_skipped_chain_artifacts() {
        let (_temp, workspace, target) = test_workspace();
        workspace
            .store()
            .write_json(
                paths::SOURCE_BUNDLE,
                &SourceBundleArtifact {
                    target: target.clone(),
                    status: StepStatus::SourceFetchFailed,
                    ..SourceBundleArtifact::default()
                },
            )
            .expect("source bundle");
        let config = test_config(&workspace, &target);
        let mut service = AuditPipelineService::new(config, workspace);

        let status = service
            .run_dependency_analysis(&target.address, &target.chain)
            .expect("run dependency");
        assert_eq!(status, StepStatus::SourceNotFetched);

        let findings: DependencyFindingsArtifact = super::super::support::read_json_if_exists(
            &service
                .workspace
                .paths()
                .resolve(paths::DEPENDENCY_FINDINGS),
        )
        .expect("findings");
        assert_eq!(findings.status, StepStatus::SourceNotFetched);

        let summary: DependencyChainChecksArtifact = super::super::support::read_json_if_exists(
            &service
                .workspace
                .paths()
                .resolve(paths::DEPENDENCY_CHAIN_CHECKS),
        )
        .expect("summary");
        assert_eq!(summary.status, ChainCheckStatus::SourceNotFetched);
    }

    #[test]
    fn bytecode_targets_record_missing_rpc_per_target() {
        let (_temp, workspace, target) = test_workspace();
        let config = test_config(&workspace, &target);
        let mut service = AuditPipelineService::new(config, workspace);

        let (path, status) = service
            .write_bytecode_targets(
                &target,
                vec![BytecodeTargetCandidate {
                    address: target.address.clone(),
                    chain: target.chain.clone(),
                    role: "target".into(),
                    name: "target".into(),
                    source_availability: SourceAvailabilityStatus::Unavailable,
                    source_unavailable_reason: Some("not verified".into()),
                    origin: "source_provider_unavailable".into(),
                    origin_evidence: vec![WorkspaceRelPath::new(paths::SOURCE_BUNDLE)],
                }],
            )
            .expect("write bytecode targets");

        assert_eq!(status, StepStatus::ConfiguredNotExecuted);
        let payload: BytecodeTargetsArtifact = super::super::support::read_json_if_exists(
            &service.workspace.paths().resolve(path.as_str()),
        )
        .expect("bytecode targets");
        assert_eq!(payload.targets.len(), 1);
        assert_eq!(
            payload.targets[0].bytecode_status,
            BytecodeFetchStatus::RpcNotConfigured
        );
        assert_eq!(
            payload.targets[0].source_availability,
            SourceAvailabilityStatus::Unavailable
        );
    }

    #[test]
    fn bytecode_targets_fetch_runtime_bytecode_when_rpc_configured() {
        let (_temp, workspace, target) = test_workspace();
        let rpc = spawn_mock_rpc_server(None);
        let mut config = test_config(&workspace, &target);
        config.rpc_url = Some(Url::parse(&rpc).expect("rpc url"));
        let mut service = AuditPipelineService::new(config, workspace);

        let (path, status) = service
            .write_bytecode_targets(
                &target,
                vec![BytecodeTargetCandidate {
                    address: target.address.clone(),
                    chain: target.chain.clone(),
                    role: "target".into(),
                    name: "target".into(),
                    source_availability: SourceAvailabilityStatus::Unavailable,
                    source_unavailable_reason: Some("not verified".into()),
                    origin: "source_provider_unavailable".into(),
                    origin_evidence: vec![WorkspaceRelPath::new(paths::SOURCE_BUNDLE)],
                }],
            )
            .expect("write bytecode targets");

        assert_eq!(status, StepStatus::Executed);
        let payload: BytecodeTargetsArtifact = super::super::support::read_json_if_exists(
            &service.workspace.paths().resolve(path.as_str()),
        )
        .expect("bytecode targets");
        assert_eq!(payload.targets.len(), 1);
        assert_eq!(
            payload.targets[0].bytecode_status,
            BytecodeFetchStatus::Fetched
        );
        let bytecode_path = payload.targets[0]
            .bytecode_artifact
            .as_ref()
            .expect("bytecode artifact");
        let bytecode = std::fs::read_to_string(service.workspace.paths().resolve(bytecode_path))
            .expect("read bytecode");
        assert_eq!(bytecode, "0x60016002\n");
    }

    #[test]
    fn closed_target_records_slot_implementation_as_bytecode_candidate() {
        let (_temp, workspace, target) = test_workspace();
        let implementation = EvmAddress::new("0x52908400098527886e0f7030069857d2e4169ee7")
            .expect("implementation address");
        let rpc = spawn_mock_rpc_server(Some(&implementation));
        let source_api = spawn_source_unavailable_api_server();
        let mut config = test_config(&workspace, &target);
        config.rpc_url = Some(Url::parse(&rpc).expect("rpc url"));
        config.source_api_base = Some(Url::parse(&source_api).expect("source api url"));
        let mut service = AuditPipelineService::new(config, workspace);

        let status = service
            .fetch_contract_source(&target.address, &target.chain)
            .expect("fetch source");

        assert_eq!(status, StepStatus::SourceUnavailable);
        let bundle: SourceBundleArtifact = super::super::support::read_json_if_exists(
            &service.workspace.paths().resolve(paths::SOURCE_BUNDLE),
        )
        .expect("source bundle");
        assert_eq!(bundle.status, StepStatus::SourceUnavailable);
        assert_eq!(
            bundle.proxy_resolution.as_ref().map(|proxy| proxy.status),
            Some(ProxyResolutionStatus::Eip1967Slots)
        );
        assert_eq!(
            bundle
                .proxy_resolution
                .as_ref()
                .and_then(|proxy| proxy.implementation.as_ref()),
            Some(&implementation)
        );
        assert_eq!(bundle.related_contracts.len(), 1);
        assert_eq!(
            bundle.related_contracts[0].status,
            DependencyFetchStatus::SourceUnavailable
        );

        let bytecode_targets: BytecodeTargetsArtifact = super::super::support::read_json_if_exists(
            &service.workspace.paths().resolve(paths::BYTECODE_TARGETS),
        )
        .expect("bytecode targets");
        assert_eq!(bytecode_targets.status, StepStatus::Executed);
        assert_eq!(bytecode_targets.targets.len(), 2);
        assert!(bytecode_targets.targets.iter().any(|item| {
            item.role == "target" && item.bytecode_status == BytecodeFetchStatus::Fetched
        }));
        assert!(bytecode_targets.targets.iter().any(|item| {
            item.role == "implementation"
                && item.address == implementation
                && item.origin == "implementation_source_unavailable"
                && item.bytecode_status == BytecodeFetchStatus::Fetched
        }));
    }

    #[test]
    fn source_unavailable_runs_target_level_chain_artifacts() {
        let (_temp, workspace, target) = test_workspace();
        workspace
            .store()
            .write_json(
                paths::SOURCE_BUNDLE,
                &SourceBundleArtifact {
                    target: target.clone(),
                    status: StepStatus::SourceUnavailable,
                    source_availability: SourceAvailabilityStatus::Unavailable,
                    source_unavailable_reason: Some("not verified".into()),
                    ..SourceBundleArtifact::default()
                },
            )
            .expect("source bundle");
        let config = test_config(&workspace, &target);
        let mut service = AuditPipelineService::new(config, workspace);

        let status = service
            .run_dependency_analysis(&target.address, &target.chain)
            .expect("run dependency");

        assert_eq!(status, StepStatus::SourceUnavailable);
        let findings: DependencyFindingsArtifact = super::super::support::read_json_if_exists(
            &service
                .workspace
                .paths()
                .resolve(paths::DEPENDENCY_FINDINGS),
        )
        .expect("findings");
        assert_eq!(findings.status, StepStatus::SourceUnavailable);

        let summary: DependencyChainChecksArtifact = super::super::support::read_json_if_exists(
            &service
                .workspace
                .paths()
                .resolve(paths::DEPENDENCY_CHAIN_CHECKS),
        )
        .expect("summary");
        assert_eq!(summary.status, ChainCheckStatus::RpcNotConfigured);
        assert!(
            summary
                .evidence_artifacts
                .iter()
                .any(|path| path.as_str() == paths::PROXY_CHECKS)
        );
    }
}
