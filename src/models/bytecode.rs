use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::models::identity::{ChainAlias, EvmAddress};
use crate::models::path::WorkspaceRelPath;
use crate::models::run::RunTarget;
use crate::models::source::SourceAvailabilityStatus;
use crate::models::step::StepStatus;

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BytecodeTargetsArtifact {
    pub target: RunTarget,
    pub status: StepStatus,
    pub rpc_url_configured: bool,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub targets: Vec<BytecodeAuditTarget>,
    pub note: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BytecodeAuditTarget {
    pub address: EvmAddress,
    pub chain: ChainAlias,
    pub role: String,
    pub name: String,
    pub source_availability: SourceAvailabilityStatus,
    pub source_unavailable_reason: Option<String>,
    pub origin: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub origin_evidence: Vec<WorkspaceRelPath>,
    pub bytecode_status: BytecodeFetchStatus,
    pub bytecode_artifact: Option<WorkspaceRelPath>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BytecodeFetchStatus {
    #[default]
    NotAttempted,
    Fetched,
    RpcNotConfigured,
    EmptyCode,
    FetchFailed,
}

impl BytecodeTargetsArtifact {
    pub fn new(
        target: RunTarget,
        rpc_url_configured: bool,
        targets: Vec<BytecodeAuditTarget>,
    ) -> Self {
        let status = aggregate_bytecode_status(&targets, rpc_url_configured);
        let note = if targets.is_empty() {
            Some("No source-unavailable bytecode audit targets were identified.".to_string())
        } else if !rpc_url_configured {
            Some("RPC is not configured; runtime bytecode could not be fetched.".to_string())
        } else {
            None
        };
        Self {
            target,
            status,
            rpc_url_configured,
            targets,
            note,
        }
    }
}

fn aggregate_bytecode_status(
    targets: &[BytecodeAuditTarget],
    rpc_url_configured: bool,
) -> StepStatus {
    if targets.is_empty() {
        return StepStatus::Executed;
    }
    if !rpc_url_configured {
        return StepStatus::ConfiguredNotExecuted;
    }
    if targets
        .iter()
        .all(|target| target.bytecode_status == BytecodeFetchStatus::Fetched)
    {
        StepStatus::Executed
    } else {
        StepStatus::ExecutedWithError
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::identity::{ChainAlias, EvmAddress};

    fn sample_target(status: BytecodeFetchStatus) -> BytecodeAuditTarget {
        BytecodeAuditTarget {
            address: EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678")
                .expect("address"),
            chain: ChainAlias::new("eth").expect("chain"),
            role: "target".into(),
            name: "target".into(),
            bytecode_status: status,
            ..BytecodeAuditTarget::default()
        }
    }

    #[test]
    fn bytecode_status_serializes_as_snake_case() {
        let json =
            serde_json::to_string(&BytecodeFetchStatus::RpcNotConfigured).expect("serialize");
        assert_eq!(json, "\"rpc_not_configured\"");
    }

    #[test]
    fn bytecode_targets_aggregate_missing_rpc_as_configured_not_executed() {
        let artifact = BytecodeTargetsArtifact::new(
            RunTarget::default(),
            false,
            vec![sample_target(BytecodeFetchStatus::RpcNotConfigured)],
        );

        assert_eq!(artifact.status, StepStatus::ConfiguredNotExecuted);
    }

    #[test]
    fn bytecode_targets_aggregate_fetch_errors_as_executed_with_error() {
        let artifact = BytecodeTargetsArtifact::new(
            RunTarget::default(),
            true,
            vec![sample_target(BytecodeFetchStatus::EmptyCode)],
        );

        assert_eq!(artifact.status, StepStatus::ExecutedWithError);
    }
}
