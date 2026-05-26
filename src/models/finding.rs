use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::models::path::WorkspaceRelPath;
use crate::models::run::RunTarget;
use crate::models::step::StepStatus;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DependencyFindingsArtifact {
    pub target: RunTarget,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub findings: Vec<DependencyFinding>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DependencyFinding {
    pub title: String,
    pub severity: FindingSeverity,
    pub confidence: FindingConfidence,
    pub summary: String,
    pub source: String,
    pub location: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DependencyChainChecksArtifact {
    pub target: RunTarget,
    pub status: ChainCheckStatus,
    pub rpc_url_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_block: Option<ChainBlockSnapshot>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub summary_signals: Vec<ExternalDependencySignal>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
    pub proxy_checks_artifact: Option<WorkspaceRelPath>,
    pub oracle_checks_artifact: Option<WorkspaceRelPath>,
    pub flash_loan_surface_artifact: Option<WorkspaceRelPath>,
    pub note: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyChecksArtifact {
    pub target: RunTarget,
    pub status: ChainCheckStatus,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub checks: Vec<ProxyCheckResult>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
    pub note: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OracleChecksArtifact {
    pub target: RunTarget,
    pub status: ChainCheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_block: Option<ChainBlockSnapshot>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub checks: Vec<OracleCheckResult>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
    pub note: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FlashLoanSurfaceArtifact {
    pub target: RunTarget,
    pub status: ChainCheckStatus,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub target_function_signals: Vec<TargetFunctionSignal>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub dependencies: Vec<FlashLoanSurfaceEntry>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChainBlockSnapshot {
    pub block_number: String,
    pub timestamp: u64,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyCheckResult {
    pub address: String,
    pub role: String,
    pub name: String,
    pub provider_is_proxy: bool,
    pub provider_implementation: Option<String>,
    pub eip1967_implementation: Option<String>,
    pub eip1967_admin: Option<String>,
    pub eip1967_beacon: Option<String>,
    pub implementation_call: Option<AddressCallResult>,
    pub admin_call: Option<AddressCallResult>,
    pub owner_call: Option<AddressCallResult>,
    pub proxiable_uuid: Option<String>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub signals: Vec<ExternalDependencySignal>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OracleCheckResult {
    pub address: String,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub candidate_hints: Vec<String>,
    pub decimals: Option<u8>,
    pub description: Option<String>,
    pub version: Option<u64>,
    pub latest_round_data: Option<ChainlinkLatestRoundData>,
    pub staleness_seconds: Option<u64>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub failed_reads: Vec<String>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub signals: Vec<ExternalDependencySignal>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FlashLoanSurfaceEntry {
    pub address: String,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub source_locations: Vec<String>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub abi_hints: Vec<String>,
    pub token_metadata: Option<TokenSurfaceMetadata>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub target_function_matches: Vec<String>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenSurfaceMetadata {
    pub symbol: Option<String>,
    pub decimals: Option<u8>,
    pub total_supply: Option<String>,
    pub balance_of_target: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetFunctionSignal {
    pub name: String,
    pub selector: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub tags: Vec<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalDependencySignal {
    pub signal: String,
    pub severity: FindingSeverity,
    pub summary: String,
    pub address: Option<String>,
    pub location: Option<String>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AddressCallResult {
    pub status: ChainCheckStatus,
    pub address: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChainlinkLatestRoundData {
    pub round_id: String,
    pub answer: String,
    pub started_at: u64,
    pub updated_at: u64,
    pub answered_in_round: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainCheckStatus {
    #[default]
    SourceNotFetched,
    Executed,
    Partial,
    RpcNotConfigured,
    CallFailed,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingConfidence {
    Low,
    #[default]
    Medium,
    High,
}

impl DependencyFindingsArtifact {
    pub fn new(target: RunTarget, status: StepStatus, findings: Vec<DependencyFinding>) -> Self {
        Self {
            target,
            status,
            findings,
        }
    }
}

impl DependencyChainChecksArtifact {
    pub fn new(target: RunTarget, status: ChainCheckStatus) -> Self {
        Self {
            target,
            status,
            ..Self::default()
        }
    }
}

impl ProxyChecksArtifact {
    pub fn new(target: RunTarget, status: ChainCheckStatus) -> Self {
        Self {
            target,
            status,
            ..Self::default()
        }
    }
}

impl OracleChecksArtifact {
    pub fn new(target: RunTarget, status: ChainCheckStatus) -> Self {
        Self {
            target,
            status,
            ..Self::default()
        }
    }
}

impl FlashLoanSurfaceArtifact {
    pub fn new(target: RunTarget, status: ChainCheckStatus) -> Self {
        Self {
            target,
            status,
            ..Self::default()
        }
    }
}

impl ChainCheckStatus {
    pub const fn artifact_status(self) -> StepStatus {
        match self {
            Self::Executed => StepStatus::Executed,
            Self::Partial | Self::CallFailed => StepStatus::ExecutedWithError,
            Self::RpcNotConfigured | Self::SourceNotFetched => StepStatus::ConfiguredNotExecuted,
        }
    }
}

impl DependencyFinding {
    pub fn new(
        title: impl Into<String>,
        severity: FindingSeverity,
        confidence: FindingConfidence,
        summary: impl Into<String>,
        source: impl Into<String>,
        location: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            severity,
            confidence,
            summary: summary.into(),
            source: source.into(),
            location: location.into(),
            evidence_artifacts: Vec::new(),
        }
    }

    pub fn with_evidence(mut self, evidence_artifacts: Vec<WorkspaceRelPath>) -> Self {
        self.evidence_artifacts = evidence_artifacts;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_findings_default_to_not_prepared() {
        let payload = DependencyFindingsArtifact::default();
        assert_eq!(payload.status, StepStatus::NotPrepared);
    }

    #[test]
    fn chain_check_status_serializes_as_snake_case() {
        let json = serde_json::to_string(&ChainCheckStatus::RpcNotConfigured)
            .expect("serialize chain check status");
        assert_eq!(json, "\"rpc_not_configured\"");
    }
}
