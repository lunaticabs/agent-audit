use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_with::skip_serializing_none;
use url::Url;

use crate::models::discovery::{DependencyDiscoveryContext, DependencyDiscoveryReport};
use crate::models::envelope::StepStatus;
use crate::models::identity::EvmAddress;
use crate::models::path::{RelativePath, WorkspaceRelPath};
use crate::models::run::RunTarget;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: RelativePath,
    pub content: String,
}

#[derive(Clone, Debug)]
pub struct SourceBundle {
    pub provider_payload: Value,
    pub normalized_payload: VerifiedSourceMetadata,
    pub files: Vec<SourceFile>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VerifiedSourceMetadata {
    pub target: RunTarget,
    pub provider: SourceProviderMetadata,
    pub contract: ContractMetadata,
    pub compiler: CompilerMetadata,
    #[serde(skip_serializing_if = "crate::serde_ext::is_json_null")]
    pub abi: Value,
    pub source_layout: String,
    pub source_meta: SourceMetadata,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub files: Vec<ArtifactSourceFile>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceBundleArtifact {
    pub target: RunTarget,
    pub status: StepStatus,
    pub note: Option<String>,
    pub error: Option<String>,
    pub error_debug: Option<String>,
    pub provider: Option<SourceProviderMetadata>,
    pub contract: Option<ContractMetadata>,
    pub compiler: Option<CompilerMetadata>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_json_null")]
    pub abi: Value,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub source_layout: String,
    pub source_meta: Option<SourceMetadata>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub files: Vec<ArtifactSourceFile>,
    pub proxy_resolution: Option<ProxyResolution>,
    pub dependency_discovery: Option<DependencyDiscoveryReport>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub dependencies: Vec<DependencyRecord>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub related_contracts: Vec<DependencyRecord>,
    pub analysis_target: Option<AnalysisTarget>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceFetchRequestArtifact {
    pub address: EvmAddress,
    pub chain: crate::models::identity::ChainAlias,
    pub source_api_base: Option<Url>,
    pub source_api_configured: bool,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub source_api_header_names: Vec<String>,
    pub rpc_url_configured: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceProviderMetadata {
    #[serde(rename = "type")]
    pub kind: String,
    pub endpoint: String,
    pub message: String,
    pub result_count: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContractMetadata {
    pub name: String,
    pub file_name: Option<RelativePath>,
    pub proxy: bool,
    pub implementation: Option<EvmAddress>,
    pub similar_match: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompilerMetadata {
    pub version: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub optimization_used: String,
    pub runs: String,
    pub evm_version: String,
    pub constructor_arguments: String,
    pub license_type: String,
    pub library: String,
    pub swarm_source: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceMetadata {
    pub language: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_json_null")]
    pub settings: Value,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactSourceFile {
    pub path: RelativePath,
    pub length: usize,
    pub original_path: Option<RelativePath>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProxyResolution {
    pub status: ProxyResolutionStatus,
    pub proxy: bool,
    pub implementation: Option<EvmAddress>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisTarget {
    pub address: EvmAddress,
    pub contract_name: String,
    pub path: RelativePath,
    pub role: String,
    pub prepared_path: Option<RelativePath>,
    pub prepared_root: Option<RelativePath>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DependencyRecord {
    pub role: String,
    pub name: String,
    pub address: EvmAddress,
    pub provider: Option<SourceProviderMetadata>,
    pub contract: Option<ContractMetadata>,
    pub compiler: Option<CompilerMetadata>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_json_null")]
    pub abi: Value,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub source_layout: String,
    pub source_meta: Option<SourceMetadata>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub files: Vec<ArtifactSourceFile>,
    pub provider_response_artifact: Option<WorkspaceRelPath>,
    pub status: DependencyFetchStatus,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub related_contracts: Vec<DependencyRecord>,
    pub discovery: Option<DependencyDiscoveryContext>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyResolutionStatus {
    #[default]
    NotAttempted,
    ProviderFlagOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyFetchStatus {
    #[default]
    FetchFailed,
    Fetched,
}

impl SourceBundleArtifact {
    pub fn not_configured(target: RunTarget) -> Self {
        Self {
            target,
            status: StepStatus::SourceApiNotConfigured,
            note: Some(
                "Configure AGENT_AUDIT_SOURCE_API_BASE to enable verified source fetching."
                    .to_string(),
            ),
            proxy_resolution: Some(ProxyResolution {
                status: ProxyResolutionStatus::NotAttempted,
                ..ProxyResolution::default()
            }),
            ..Self::default()
        }
    }

    pub fn fetch_failed(target: RunTarget, error: String, error_debug: String) -> Self {
        Self {
            target,
            status: StepStatus::SourceFetchFailed,
            error: Some(error),
            error_debug: Some(error_debug),
            proxy_resolution: Some(ProxyResolution {
                status: ProxyResolutionStatus::NotAttempted,
                ..ProxyResolution::default()
            }),
            ..Self::default()
        }
    }

    pub fn from_verified_source(metadata: VerifiedSourceMetadata) -> Self {
        Self {
            target: metadata.target.clone(),
            status: StepStatus::SourceFetched,
            provider: Some(metadata.provider),
            contract: Some(metadata.contract),
            compiler: Some(metadata.compiler),
            abi: metadata.abi,
            source_layout: metadata.source_layout,
            source_meta: Some(metadata.source_meta),
            files: metadata.files,
            ..Self::default()
        }
    }

    pub fn is_fetched(&self) -> bool {
        self.status == StepStatus::SourceFetched
    }
}

impl AnalysisTarget {
    pub fn with_prepared(
        mut self,
        prepared_path: impl Into<RelativePath>,
        prepared_root: impl Into<RelativePath>,
    ) -> Self {
        self.prepared_path = Some(prepared_path.into());
        self.prepared_root = Some(prepared_root.into());
        self
    }
}

impl DependencyRecord {
    pub fn is_fetched(&self) -> bool {
        self.status == DependencyFetchStatus::Fetched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_bundle_defaults_to_not_prepared_status() {
        let payload = SourceBundleArtifact::default();
        assert_eq!(payload.status, StepStatus::NotPrepared);
    }

    #[test]
    fn dependency_record_defaults_to_fetch_failed_status() {
        let payload = DependencyRecord::default();
        assert_eq!(payload.status, DependencyFetchStatus::FetchFailed);
    }
}
