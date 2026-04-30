use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::models::artifact::ArtifactRecord;
use crate::models::envelope::StepStatus;
use crate::models::identity::RunId;
use crate::models::path::{RelativePath, WorkspaceRelPath};
use crate::models::run::RunTarget;
use crate::models::source::AnalysisTarget;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunArtifactHeader {
    pub target: RunTarget,
    pub run_id: RunId,
    pub status: StepStatus,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MaterialStatusSnapshot {
    pub source_fetch_status: StepStatus,
    pub dependency_analysis_status: StepStatus,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolingManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    pub source_fetch_status: StepStatus,
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
    pub status: StepStatus,
    pub manifest_path: WorkspaceRelPath,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MaterialsManifest {
    pub target: RunTarget,
    pub run_id: RunId,
    pub statuses: MaterialStatusSnapshot,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub inputs: Vec<WorkspaceRelPath>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub core_materials: Vec<WorkspaceRelPath>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub optional_tool_artifacts: Vec<WorkspaceRelPath>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub artifact_records: Vec<ArtifactRecord>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlitherInputsArtifact {
    pub status: StepStatus,
    pub working_dir: RelativePath,
    pub base_path: RelativePath,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub include_paths: Vec<RelativePath>,
    pub remappings_file: RelativePath,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub remappings: Vec<String>,
    pub solc_args: String,
    pub target_path: RelativePath,
    pub prepared_target: RelativePath,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlitherBuildManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    pub slither_project_root: Option<WorkspaceRelPath>,
    pub analysis_target: Option<AnalysisTarget>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub compiler_version: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub solc_version: String,
    pub solc_select: Option<SolcSelectStatus>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub linked_source_entries: Vec<SourceLink>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub node_modules_links: Vec<NodeModuleLink>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub remappings: Vec<String>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub solc_args: String,
    pub config_path: Option<WorkspaceRelPath>,
    pub preferred_target: Option<RelativePath>,
    pub preferred_working_dir: Option<WorkspaceRelPath>,
    pub preferred_source_root: Option<RelativePath>,
    pub note: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FoundryBuildManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    pub project_root: Option<WorkspaceRelPath>,
    pub analysis_target: Option<AnalysisTarget>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub source_links: Vec<SourceLink>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub node_modules_links: Vec<NodeModuleLink>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub compiler_version: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub solc_version: String,
    pub optimizer_enabled: bool,
    pub optimizer_runs: u64,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evm_version: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub remappings: Vec<String>,
    pub remappings_path: Option<WorkspaceRelPath>,
    pub foundry_toml_path: Option<WorkspaceRelPath>,
    pub preferred_working_dir: Option<WorkspaceRelPath>,
    pub preferred_target: Option<RelativePath>,
    pub preferred_source_root: Option<RelativePath>,
    pub test_dir: Option<WorkspaceRelPath>,
    pub script_dir: Option<WorkspaceRelPath>,
    pub note: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EchidnaBuildManifest {
    #[serde(flatten)]
    pub header: RunArtifactHeader,
    pub project_root: Option<WorkspaceRelPath>,
    pub analysis_target: Option<AnalysisTarget>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub source_links: Vec<SourceLink>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub node_modules_links: Vec<NodeModuleLink>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub compiler_version: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub solc_version: String,
    pub optimizer_enabled: bool,
    pub optimizer_runs: u64,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evm_version: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub remappings: Vec<String>,
    pub config_path: Option<WorkspaceRelPath>,
    pub preferred_working_dir: Option<WorkspaceRelPath>,
    pub preferred_target: Option<RelativePath>,
    pub preferred_source_root: Option<RelativePath>,
    pub harness_dir: Option<WorkspaceRelPath>,
    pub note: Option<String>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceLink {
    pub path: RelativePath,
    pub target: WorkspaceRelPath,
    pub kind: Option<SourceLinkKind>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NodeModuleLink {
    pub alias: String,
    pub version: String,
    pub link_path: WorkspaceRelPath,
    pub target: WorkspaceRelPath,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLinkKind {
    #[default]
    File,
    Directory,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SolcSelectStatus {
    pub requested_version: String,
    pub is_installed: bool,
    pub current_version: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub available_versions: Vec<String>,
    pub recommended_action: String,
    pub command_status: ToolCommandStatus,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub stderr_preview: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCommandStatus {
    #[default]
    Error,
    Ok,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_status_snapshot_defaults_to_not_prepared() {
        let snapshot = MaterialStatusSnapshot::default();
        assert_eq!(snapshot.source_fetch_status, StepStatus::NotPrepared);
        assert_eq!(snapshot.dependency_analysis_status, StepStatus::NotPrepared);
    }

    #[test]
    fn tool_command_status_serializes_as_snake_case() {
        let json = serde_json::to_string(&ToolCommandStatus::Ok).expect("serialize command status");
        assert_eq!(json, "\"ok\"");
    }
}
