use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use crate::analysis::dependencies::analyze_dependencies;
use crate::analysis::discovery::discover_dependencies;
use crate::error::AppResult;
use crate::models::discovery::{DependencyCandidate, DependencyDiscoveryContext};
use crate::models::envelope::StepStatus;
use crate::models::finding::DependencyFindingsArtifact;
use crate::models::identity::{ChainAlias, EvmAddress};
use crate::models::path::RelativePath;
use crate::models::run::RunTarget;
use crate::models::source::{
    AnalysisTarget, ArtifactSourceFile, ContractMetadata, DependencyFetchStatus, DependencyRecord,
    ProxyResolution, ProxyResolutionStatus, SourceBundleArtifact, SourceFetchRequestArtifact,
    SourceFile, VerifiedSourceMetadata,
};
use crate::services::source_provider::{fetch_verified_source, sanitize_dependency_name};

use super::AuditPipelineService;

impl AuditPipelineService {
    pub fn fetch_contract_source(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<StepStatus> {
        let request_payload = SourceFetchRequestArtifact {
            address: address.clone(),
            chain: chain.clone(),
            source_api_base: self.config.source_api_base.clone(),
            source_api_configured: self.config.source_api_base.is_some(),
            source_api_header_names: self.config.source_api_headers.keys().cloned().collect(),
            rpc_url_configured: self.config.rpc_url.is_some(),
        };
        let request_path = self
            .workspace
            .write_json("input/source_request.json", &request_payload)?;
        let target = RunTarget::new(address.clone(), chain.clone());

        let Some(base_url) = self.config.source_api_base.clone() else {
            let bundle_path = self.workspace.write_json(
                "artifacts/source_bundle.json",
                &SourceBundleArtifact::not_configured(target),
            )?;
            self.record(
                "fetch_contract_source",
                &request_path,
                "request",
                "configured_not_executed",
                "Persisted source fetch request metadata.",
            );
            self.record(
                "fetch_contract_source",
                &bundle_path,
                "artifact",
                "configured_not_executed",
                "Skipped source fetch because the source API is not configured.",
            );
            return Ok(StepStatus::SourceApiNotConfigured);
        };

        let bundle = match fetch_verified_source(
            &base_url,
            self.config.source_api_key.as_deref(),
            &self.config.source_api_headers,
            address,
            chain,
        ) {
            Ok(bundle) => bundle,
            Err(error) => {
                let bundle_path = self.workspace.write_json(
                    "artifacts/source_bundle.json",
                    &SourceBundleArtifact::fetch_failed(
                        target,
                        error.to_string(),
                        format!("{error:?}"),
                    ),
                )?;
                self.record(
                    "fetch_contract_source",
                    &request_path,
                    "request",
                    "executed_with_error",
                    "Persisted source fetch request metadata.",
                );
                self.record(
                    "fetch_contract_source",
                    &bundle_path,
                    "artifact",
                    "executed_with_error",
                    "Source fetch failed; inspect the stored error payload.",
                );
                return Ok(StepStatus::SourceFetchFailed);
            }
        };

        let proxy_contract = bundle.normalized_payload.contract.clone();
        let implementation_address = proxy_contract.implementation.clone();

        let raw_response_path = self.workspace.write_json(
            "artifacts/source_provider_response.json",
            &bundle.provider_payload,
        )?;
        let primary_sources =
            self.write_fetched_source_files(&bundle.files, None, "Stored a fetched source file.")?;

        let mut related_contracts = Vec::new();
        if proxy_contract.proxy
            && implementation_address
                .as_ref()
                .is_some_and(|implementation| implementation != address)
        {
            related_contracts.push(
                self.fetch_dependency_bundle_record(
                    implementation_address
                        .as_ref()
                        .expect("checked implementation presence"),
                    chain,
                    "implementation",
                    "implementation",
                    &RelativePath::new("implementation"),
                )?,
            );
        }

        let source_map_for_discovery = self.source_map_for_discovery(&primary_sources)?;
        let dependency_discovery =
            discover_dependencies(&bundle.normalized_payload, &source_map_for_discovery);
        let dependencies = self.fetch_discovered_dependencies(
            dependency_discovery.merged_candidates.clone(),
            address,
            chain,
            if implementation_address.is_none() {
                BTreeSet::new()
            } else {
                BTreeSet::from([implementation_address
                    .as_ref()
                    .expect("checked implementation presence")
                    .as_lowercase()])
            },
        )?;

        let analysis_target = analysis_target_from_bundle(
            address,
            &proxy_contract,
            &primary_sources,
            &related_contracts,
        );

        let mut bundle_payload =
            SourceBundleArtifact::from_verified_source(bundle.normalized_payload);
        bundle_payload.proxy_resolution = Some(ProxyResolution {
            status: ProxyResolutionStatus::ProviderFlagOnly,
            proxy: proxy_contract.proxy,
            implementation: proxy_contract.implementation,
        });
        bundle_payload.dependency_discovery = Some(dependency_discovery);
        bundle_payload.dependencies = dependencies;
        bundle_payload.related_contracts = related_contracts;
        bundle_payload.analysis_target = Some(analysis_target);

        let bundle_path = self
            .workspace
            .write_json("artifacts/source_bundle.json", &bundle_payload)?;

        self.record(
            "fetch_contract_source",
            &request_path,
            "request",
            "executed",
            "Persisted source fetch request metadata.",
        );
        self.record(
            "fetch_contract_source",
            &raw_response_path,
            "artifact",
            "executed",
            "Stored the raw source provider response.",
        );
        self.record(
            "fetch_contract_source",
            &bundle_path,
            "artifact",
            "executed",
            "Fetched and normalized verified source metadata.",
        );
        Ok(StepStatus::SourceFetched)
    }

    pub fn run_dependency_analysis(
        &mut self,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<StepStatus> {
        let bundle_payload = self.load_source_bundle_payload()?;
        if !bundle_payload.is_fetched() {
            let findings_path = self.workspace.write_json(
                "artifacts/dependency_findings.json",
                &DependencyFindingsArtifact::new(
                    RunTarget::new(address.clone(), chain.clone()),
                    StepStatus::SourceNotFetched,
                    Vec::new(),
                ),
            )?;
            self.record(
                "run_dependency_analysis",
                &findings_path,
                "artifact",
                "configured_not_executed",
                "Skipped dependency analysis because source fetching did not complete.",
            );
            return Ok(StepStatus::SourceNotFetched);
        }

        let findings = analyze_dependencies(&bundle_payload, &self.workspace.root);
        let status = StepStatus::Executed;
        let findings_path = self.workspace.write_json(
            "artifacts/dependency_findings.json",
            &DependencyFindingsArtifact::new(
                RunTarget::new(address.clone(), chain.clone()),
                status,
                findings,
            ),
        )?;
        self.record(
            "run_dependency_analysis",
            &findings_path,
            "artifact",
            status.as_str(),
            "Analyzed fetched dependencies for high-signal role-specific findings.",
        );
        Ok(status)
    }

    fn source_map_for_discovery(
        &self,
        primary_sources: &[ArtifactSourceFile],
    ) -> AppResult<BTreeMap<String, String>> {
        let mut source_map_for_discovery = BTreeMap::new();
        for item in primary_sources {
            let relative_path = item.path.as_str();
            let file_path = self.workspace.root.join("sources").join(relative_path);
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
                original_path: Some(source_file.path.clone()),
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
        let Some(base_url) = self.config.source_api_base.clone() else {
            return Ok(DependencyRecord {
                role: role.to_string(),
                name: name.to_string(),
                address: address.clone(),
                status: DependencyFetchStatus::FetchFailed,
                error: Some("missing source API base".to_string()),
                ..DependencyRecord::default()
            });
        };

        let bundle = match fetch_verified_source(
            &base_url,
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
                    error: Some(error.to_string()),
                    ..DependencyRecord::default()
                });
            }
        };

        let response_artifact = self.workspace.write_json(
            &format!(
                "artifacts/source_provider_response_{}.json",
                prefix.as_str().replace('/', "_")
            ),
            &bundle.provider_payload,
        )?;
        let written_files = self.write_fetched_source_files(
            &bundle.files,
            Some(prefix),
            "Stored a fetched dependency source file.",
        )?;

        let mut record = DependencyRecord {
            role: role.to_string(),
            name: name.to_string(),
            address: address.clone(),
            provider: Some(bundle.normalized_payload.provider.clone()),
            contract: Some(bundle.normalized_payload.contract.clone()),
            compiler: Some(bundle.normalized_payload.compiler.clone()),
            abi: bundle.normalized_payload.abi.clone(),
            source_layout: bundle.normalized_payload.source_layout.clone(),
            source_meta: Some(bundle.normalized_payload.source_meta.clone()),
            files: written_files,
            provider_response_artifact: Some(response_artifact.clone()),
            status: DependencyFetchStatus::Fetched,
            related_contracts: Vec::new(),
            ..DependencyRecord::default()
        };
        self.record(
            "fetch_contract_source",
            &response_artifact,
            "artifact",
            "executed",
            "Stored the raw dependency provider response.",
        );

        let contract = bundle.normalized_payload.contract;
        let implementation_address = contract.implementation.clone();
        if contract.proxy
            && implementation_address
                .as_ref()
                .is_some_and(|implementation| implementation != address)
        {
            let nested = self.fetch_dependency_bundle_record(
                implementation_address
                    .as_ref()
                    .expect("checked implementation presence"),
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
        candidates: Vec<DependencyCandidate>,
        target_address: &EvmAddress,
        chain: &ChainAlias,
        skip_addresses: BTreeSet<String>,
    ) -> AppResult<Vec<DependencyRecord>> {
        let mut records = Vec::new();
        let mut seen = BTreeSet::new();
        seen.insert(target_address.as_lowercase());
        seen.extend(skip_addresses);
        for item in candidates {
            let address = item.address;
            let address_key = address.as_lowercase();
            if seen.contains(&address_key) {
                continue;
            }
            seen.insert(address_key.clone());
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
            let mut record =
                self.fetch_dependency_bundle_record(&address, chain, role, name, &prefix)?;
            record.discovery = Some(DependencyDiscoveryContext {
                sources: item.sources,
                internal_type: item.internal_type,
                solidity_type: item.solidity_type,
                file: item.file,
            });
            records.push(record);
        }
        Ok(records)
    }
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

    let preferred_path = primary_contract.file_name.clone();
    let first_primary_path = if preferred_path
        .as_ref()
        .is_some_and(|preferred| primary_files.iter().any(|item| item.path == *preferred))
    {
        preferred_path.unwrap_or_default()
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
    let preferred_path = bundle
        .contract
        .as_ref()
        .and_then(|contract| contract.file_name.clone());
    if preferred_path
        .as_ref()
        .is_some_and(|preferred| record_for_path(bundle, preferred).is_some())
    {
        let preferred_path = preferred_path.unwrap_or_default();
        return AnalysisTarget {
            address: bundle.target.address.clone(),
            contract_name: bundle
                .contract
                .as_ref()
                .map(|contract| contract.name.clone())
                .unwrap_or_default(),
            path: preferred_path.clone(),
            role: "target".to_string(),
            prepared_path: Some(preferred_path),
            ..AnalysisTarget::default()
        };
    }

    if let Some(analysis_target) = bundle.analysis_target.as_ref() {
        return AnalysisTarget {
            address: analysis_target.address.clone(),
            contract_name: analysis_target.contract_name.clone(),
            path: analysis_target.path.clone(),
            role: analysis_target.role.clone(),
            prepared_path: Some(analysis_target.path.clone()),
            ..AnalysisTarget::default()
        };
    }

    let first_path = bundle
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
        path: first_path.clone(),
        role: "target".to_string(),
        prepared_path: Some(first_path),
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
