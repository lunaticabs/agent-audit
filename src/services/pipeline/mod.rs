mod materials;
mod source;
mod support;
mod tooling;

use crate::config::AppConfig;
use crate::error::AppResult;
use crate::models::artifact::{ArtifactIndex, ArtifactRecord};
use crate::models::source::SourceBundleArtifact;
use crate::workspace::RunWorkspace;

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

    pub fn write_artifact_index(&self) -> AppResult<String> {
        self.workspace.write_json(
            "artifacts/artifact_index.json",
            &ArtifactIndex {
                run_id: self.workspace.run_id.clone(),
                artifacts: self.artifacts.clone(),
            },
        )
    }

    fn record(&mut self, step: &str, path: &str, kind: &str, status: &str, summary: &str) {
        self.artifacts
            .retain(|item| !(item.path == path && item.step == step && item.kind == kind));
        self.artifacts.push(ArtifactRecord {
            step: step.to_string(),
            path: path.to_string(),
            kind: kind.to_string(),
            status: status.to_string(),
            summary: summary.to_string(),
        });
    }
}
