use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::path::PathBuf;
use std::str::FromStr;

use crate::models::identity::{ChainAlias, EvmAddress, RunId};
use crate::models::path::WorkspaceRelPath;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Completed,
    RetryableError,
    FatalError,
    PreconditionMissing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    NotPrepared,
    Prepared,
    Executed,
    SourceFetched,
    SourceFetchFailed,
    SourceNotFetched,
    SourceFilesMissing,
    SourceApiNotConfigured,
}

impl StepStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotPrepared => "not_prepared",
            Self::Prepared => "prepared",
            Self::Executed => "executed",
            Self::SourceFetched => "source_fetched",
            Self::SourceFetchFailed => "source_fetch_failed",
            Self::SourceNotFetched => "source_not_fetched",
            Self::SourceFilesMissing => "source_files_missing",
            Self::SourceApiNotConfigured => "source_api_not_configured",
        }
    }
}

impl FromStr for StepStatus {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "not_prepared" => Ok(Self::NotPrepared),
            "prepared" => Ok(Self::Prepared),
            "executed" => Ok(Self::Executed),
            "source_fetched" => Ok(Self::SourceFetched),
            "source_fetch_failed" => Ok(Self::SourceFetchFailed),
            "source_not_fetched" => Ok(Self::SourceNotFetched),
            "source_files_missing" => Ok(Self::SourceFilesMissing),
            "source_api_not_configured" => Ok(Self::SourceApiNotConfigured),
            _ => Err("unknown step status"),
        }
    }
}

#[skip_serializing_none]
#[derive(Clone, Debug, Serialize)]
pub struct CommandEnvelope<T> {
    pub ok: bool,
    pub status: CommandStatus,
    pub retryable: bool,
    pub run_id: Option<RunId>,
    pub run_persisted: bool,
    pub payload: Option<T>,
    pub error: Option<EnvelopeError>,
    pub next_action: NextAction,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvelopeError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum NextAction {
    #[serde(rename = "continue")]
    Continue,
    #[serde(rename = "stop")]
    Stop { command: Option<String> },
    #[serde(rename = "run_prerequisite")]
    RunPrerequisite { command: String },
    #[serde(rename = "retry_same_command")]
    RetrySameCommand {
        command: String,
        retry_after_sec: u64,
        max_retries: u64,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct SyncRunPayload {
    pub status: CommandStatus,
    pub file_count: usize,
    pub total_size_bytes: usize,
    pub upserted_file_records: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct StepPayload {
    pub run_id: RunId,
    pub run_dir: PathBuf,
    pub step: String,
    pub status: StepStatus,
    pub artifact_index: WorkspaceRelPath,
    pub init_run: Option<InitRunDetails>,
    pub fetch_source: Option<FetchSourceDetails>,
    pub prepare_slither: Option<PrepareSlitherDetails>,
    pub aggregate_materials: Option<AggregateMaterialsDetails>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InitRunDetails {
    pub address: EvmAddress,
    pub chain: ChainAlias,
    pub source_fetch_status: StepStatus,
    pub dependency_analysis_status: StepStatus,
    pub tooling_status: StepStatus,
    pub tooling_manifest_path: WorkspaceRelPath,
    pub materials_manifest_path: WorkspaceRelPath,
    pub slither_build_manifest_path: WorkspaceRelPath,
    pub foundry_build_manifest_path: WorkspaceRelPath,
    pub echidna_build_manifest_path: WorkspaceRelPath,
}

#[derive(Clone, Debug, Serialize)]
pub struct FetchSourceDetails {
    pub tooling_status: StepStatus,
    pub tooling_manifest_path: WorkspaceRelPath,
    pub slither_build_manifest_path: WorkspaceRelPath,
    pub foundry_build_manifest_path: WorkspaceRelPath,
    pub echidna_build_manifest_path: WorkspaceRelPath,
}

#[derive(Clone, Debug, Serialize)]
pub struct PrepareSlitherDetails {
    pub slither_build_manifest_path: WorkspaceRelPath,
    pub slither_project_root: WorkspaceRelPath,
}

#[derive(Clone, Debug, Serialize)]
pub struct AggregateMaterialsDetails {
    pub materials_manifest_path: WorkspaceRelPath,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_status_serializes_as_snake_case() {
        let json = serde_json::to_string(&StepStatus::SourceApiNotConfigured)
            .expect("serialize step status");
        assert_eq!(json, "\"source_api_not_configured\"");
    }
}
