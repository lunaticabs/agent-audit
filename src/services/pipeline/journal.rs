use crate::error::AppResult;
use crate::models::artifact::{
    ArtifactIndex, ArtifactKind, ArtifactRecord, ArtifactStatus, ArtifactStep,
};
use crate::models::path::WorkspaceRelPath;
use crate::workspace::{RunWorkspace, paths};

pub struct ArtifactJournal {
    records: Vec<ArtifactRecord>,
}

impl ArtifactJournal {
    pub fn load(workspace: &RunWorkspace) -> Self {
        Self {
            records: super::support::load_existing_artifacts(workspace),
        }
    }

    pub fn write_index(&self, workspace: &RunWorkspace) -> AppResult<WorkspaceRelPath> {
        workspace.store().write_json(
            paths::ARTIFACT_INDEX,
            &ArtifactIndex {
                run_id: workspace.run_id().clone(),
                artifacts: self.records.clone(),
            },
        )
    }

    pub fn record(
        &mut self,
        step: ArtifactStep,
        path: &WorkspaceRelPath,
        kind: ArtifactKind,
        status: ArtifactStatus,
        summary: &str,
    ) {
        self.records
            .retain(|item| !(item.path == *path && item.step == step && item.kind == kind));
        self.records.push(ArtifactRecord {
            step,
            path: path.clone(),
            kind,
            status,
            summary: summary.to_string(),
        });
    }

    pub fn records(&self) -> &[ArtifactRecord] {
        &self.records
    }
}
