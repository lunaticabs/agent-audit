use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use serde::Serialize;

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
    ProxyResolution, ProxyResolutionStatus, SourceBundleArtifact, SourceFile,
    VerifiedSourceMetadata,
};
use crate::services::source_provider::{fetch_verified_source, sanitize_dependency_name};

use super::AuditPipelineService;

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
        let request_path = self.workspace.write_json(
            "input/source_request.json",
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
            let bundle_path = self.workspace.write_json(
                "artifacts/source_bundle.json",
                &SourceBundleArtifact::not_configured(RunTarget::new(
                    address.clone(),
                    chain.clone(),
                )),
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
            base_url,
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
                        RunTarget::new(address.clone(), chain.clone()),
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

        let proxy_contract = &bundle.normalized_payload.contract;
        let provider_proxy = proxy_contract.proxy;
        let provider_implementation = proxy_contract.implementation.clone();
        let implementation_address = provider_implementation
            .as_ref()
            .filter(|implementation| *implementation != address);

        let raw_response_path = self.workspace.write_json(
            "artifacts/source_provider_response.json",
            &bundle.provider_payload,
        )?;
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
                error: Some("missing source API base".to_string()),
                ..DependencyRecord::default()
            });
        };

        let bundle = match fetch_verified_source(
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
                    error: Some(error.to_string()),
                    ..DependencyRecord::default()
                });
            }
        };

        let response_artifact = self.workspace.write_json(
            format!(
                "artifacts/source_provider_response_{}.json",
                prefix.as_str().replace('/', "_")
            ),
            &bundle.provider_payload,
        )?;
        self.record(
            "fetch_contract_source",
            &response_artifact,
            "artifact",
            "executed",
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
