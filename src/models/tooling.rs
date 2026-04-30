use serde::{Deserialize, Serialize};

use crate::models::artifact::ArtifactRecord;
use crate::models::run::RunTarget;
use crate::models::source::AnalysisTarget;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunArtifactHeader {
    pub target: RunTarget,
    pub run_id: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MaterialStatusSnapshot {
    pub source_fetch_status: String,
    pub dependency_analysis_status: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolingManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    pub source_fetch_status: String,
    pub workspaces: ToolWorkspaceManifestSet,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolWorkspaceManifestSet {
    pub slither: ToolWorkspaceManifest,
    pub foundry: ToolWorkspaceManifest,
    pub echidna: ToolWorkspaceManifest,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolWorkspaceManifest {
    pub status: String,
    pub manifest_path: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MaterialsManifest {
    pub target: RunTarget,
    pub run_id: String,
    pub statuses: MaterialStatusSnapshot,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub core_materials: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_tool_artifacts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_records: Vec<ArtifactRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SlitherInputsArtifact {
    pub status: String,
    pub working_dir: String,
    pub base_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_paths: Vec<String>,
    pub remappings_file: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remappings: Vec<String>,
    pub solc_args: String,
    pub target_path: String,
    pub prepared_target: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SlitherBuildManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slither_project_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_target: Option<AnalysisTarget>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub compiler_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub solc_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solc_select: Option<SolcSelectStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_source_entries: Vec<SourceLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_modules_links: Vec<NodeModuleLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remappings: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub solc_args: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub config_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_target: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_working_dir: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_source_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FoundryBuildManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_target: Option<AnalysisTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_links: Vec<SourceLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_modules_links: Vec<NodeModuleLink>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub compiler_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub solc_version: String,
    pub optimizer_enabled: bool,
    pub optimizer_runs: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evm_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remappings: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remappings_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub foundry_toml_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_working_dir: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_target: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_source_root: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub test_dir: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub script_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EchidnaBuildManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analysis_target: Option<AnalysisTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_links: Vec<SourceLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_modules_links: Vec<NodeModuleLink>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub compiler_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub solc_version: String,
    pub optimizer_enabled: bool,
    pub optimizer_runs: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub evm_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remappings: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub config_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_working_dir: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_target: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preferred_source_root: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub harness_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SourceLink {
    pub path: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<SourceLinkKind>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NodeModuleLink {
    pub alias: String,
    pub version: String,
    pub link_path: String,
    pub target: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLinkKind {
    #[default]
    File,
    Directory,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SolcSelectStatus {
    pub requested_version: String,
    pub is_installed: bool,
    pub current_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_versions: Vec<String>,
    pub recommended_action: String,
    pub command_status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr_preview: String,
}
