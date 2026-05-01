use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::path::PathBuf;

use crate::models::command::CommandName;
use crate::models::identity::{ChainAlias, EvmAddress, RunId};
use crate::models::path::WorkspaceRelPath;
use crate::models::step::StepStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    Completed,
    RetryableError,
    FatalError,
    PreconditionMissing,
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
    pub step: CommandName,
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
