use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::models::path::WorkspaceRelPath;
use crate::models::tooling::RunArtifactHeader;

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BytecodeArtifact {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    pub rpc_url_configured: bool,
    pub block_tag: String,
    pub runtime_bytecode: Option<String>,
    pub runtime_bytecode_file: Option<WorkspaceRelPath>,
    pub code_hash: Option<String>,
    pub byte_length: Option<usize>,
    pub error: Option<String>,
    pub note: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SelectorIndexArtifact {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub selectors: Vec<SelectorEntry>,
    pub source_artifact: Option<WorkspaceRelPath>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SelectorEntry {
    pub selector: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub offsets: Vec<usize>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub signature_hints: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageProbePlanArtifact {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub probes: Vec<StorageProbe>,
    pub note: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageProbe {
    pub name: String,
    pub slot: String,
    pub purpose: String,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HeimdallManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    pub version: Option<String>,
    pub input_bytecode_file: Option<WorkspaceRelPath>,
    pub input_bytecode_artifact: Option<WorkspaceRelPath>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub commands: Vec<HeimdallCommandRecord>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub output_artifacts: Vec<WorkspaceRelPath>,
    pub note: Option<String>,
    pub error: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HeimdallCommandRecord {
    pub kind: String,
    pub command: Vec<String>,
    pub exit_status: Option<i32>,
    pub stdout_path: Option<WorkspaceRelPath>,
    pub stderr_path: Option<WorkspaceRelPath>,
    pub stdout_preview: Option<String>,
    pub stderr_preview: Option<String>,
}
