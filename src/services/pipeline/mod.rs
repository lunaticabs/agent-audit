mod materials;
mod source;
mod support;
mod tooling;

use serde::Serialize;

use crate::config::AppConfig;
use crate::error::AppResult;
use crate::models::artifact::ArtifactRecord;
use crate::models::identity::RunId;
use crate::models::path::WorkspaceRelPath;
use crate::models::source::SourceBundleArtifact;
use crate::workspace::RunWorkspace;

#[derive(Serialize)]
struct ArtifactIndexRef<'a> {
    run_id: &'a RunId,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    artifacts: Vec<&'a ArtifactRecord>,
}

pub struct AuditPipelineService {
    pub config: AppConfig,
    pub workspace: RunWorkspace,
    pub artifacts: Vec<ArtifactRecord>,
}

impl AuditPipelineService {
    pub fn new(config: AppConfig, workspace: RunWorkspace) -> Self {
        let artifacts = support::load_existing_artifacts(&workspace);
        Self {
            config,
            workspace,
            artifacts,
        }
    }

    pub fn load_source_bundle_payload(&self) -> AppResult<SourceBundleArtifact> {
        support::read_json_if_exists(&self.workspace.root.join("artifacts/source_bundle.json"))
    }

    pub fn write_artifact_index(&self) -> AppResult<WorkspaceRelPath> {
        let artifacts = self.artifacts.iter().collect::<Vec<_>>();
        self.workspace.write_json(
            "artifacts/artifact_index.json",
            &ArtifactIndexRef {
                run_id: &self.workspace.run_id,
                artifacts,
            },
        )
    }

    fn record(
        &mut self,
        step: &str,
        path: &WorkspaceRelPath,
        kind: &str,
        status: &str,
        summary: &str,
    ) {
        self.artifacts
            .retain(|item| !(item.path == *path && item.step == step && item.kind == kind));
        self.artifacts.push(ArtifactRecord {
            step: step.to_string(),
            path: path.clone(),
            kind: kind.to_string(),
            status: status.to_string(),
            summary: summary.to_string(),
        });
    }
}
