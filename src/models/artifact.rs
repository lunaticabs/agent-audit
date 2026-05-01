use serde::{Deserialize, Serialize};

use crate::models::identity::RunId;
use crate::models::path::WorkspaceRelPath;
pub use crate::models::step::StepStatus as ArtifactStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStep {
    FetchContractSource,
    RunDependencyAnalysis,
    PrepareSlitherProject,
    PrepareToolingWorkspaces,
    PrepareFoundryProject,
    PrepareEchidnaProject,
    AggregateMaterials,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Request,
    Source,
    Artifact,
    Prep,
    Report,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub step: ArtifactStep,
    pub path: WorkspaceRelPath,
    pub kind: ArtifactKind,
    pub status: ArtifactStatus,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactIndex {
    pub run_id: RunId,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub artifacts: Vec<ArtifactRecord>,
}

#[cfg(test)]
mod tests {
    use super::{ArtifactKind, ArtifactStatus, ArtifactStep};

    #[test]
    fn artifact_step_serializes_as_snake_case() {
        let json = serde_json::to_string(&ArtifactStep::PrepareToolingWorkspaces)
            .expect("serialize artifact step");
        assert_eq!(json, "\"prepare_tooling_workspaces\"");
    }

    #[test]
    fn artifact_status_step_uses_nested_step_status_shape() {
        let json = serde_json::to_string(&ArtifactStatus::Prepared).expect("serialize");
        assert_eq!(json, "\"prepared\"");
    }

    #[test]
    fn artifact_kind_serializes_as_snake_case() {
        let json = serde_json::to_string(&ArtifactKind::Request).expect("serialize artifact kind");
        assert_eq!(json, "\"request\"");
    }
}
