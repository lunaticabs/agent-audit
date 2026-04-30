use serde::Serialize;
use serde_with::skip_serializing_none;

#[skip_serializing_none]
#[derive(Clone, Debug, Serialize)]
pub struct CommandEnvelope<T> {
    pub ok: bool,
    pub status: String,
    pub retryable: bool,
    pub run_id: String,
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

#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncRunPayload {
    pub status: String,
    pub file_count: usize,
    pub total_size_bytes: usize,
    pub upserted_file_records: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(default)]
pub struct StepPayload {
    pub run_id: String,
    pub run_dir: String,
    pub step: String,
    pub status: String,
    pub artifact_index: String,
    pub init_run: Option<InitRunDetails>,
    pub fetch_source: Option<FetchSourceDetails>,
    pub prepare_slither: Option<PrepareSlitherDetails>,
    pub aggregate_materials: Option<AggregateMaterialsDetails>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct InitRunDetails {
    pub address: String,
    pub chain: String,
    pub source_fetch_status: String,
    pub dependency_analysis_status: String,
    pub tooling_status: String,
    pub tooling_manifest_path: String,
    pub materials_manifest_path: String,
    pub slither_build_manifest_path: String,
    pub foundry_build_manifest_path: String,
    pub echidna_build_manifest_path: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct FetchSourceDetails {
    pub tooling_status: String,
    pub tooling_manifest_path: String,
    pub slither_build_manifest_path: String,
    pub foundry_build_manifest_path: String,
    pub echidna_build_manifest_path: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct PrepareSlitherDetails {
    pub slither_build_manifest_path: String,
    pub slither_project_root: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct AggregateMaterialsDetails {
    pub materials_manifest_path: String,
}
