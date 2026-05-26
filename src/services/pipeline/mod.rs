mod dependency_chain;
mod journal;
mod materials;
mod source;
mod support;
mod tooling;

use crate::config::AppConfig;
use crate::error::AppResult;
use crate::models::source::SourceBundleArtifact;
use crate::workspace::{RunWorkspace, paths};

use self::journal::ArtifactJournal;

pub struct AuditPipelineService {
    pub config: AppConfig,
    pub workspace: RunWorkspace,
    journal: ArtifactJournal,
}

impl AuditPipelineService {
    pub fn new(config: AppConfig, workspace: RunWorkspace) -> Self {
        let journal = ArtifactJournal::load(&workspace);
        Self {
            config,
            workspace,
            journal,
        }
    }

    pub fn load_source_bundle_payload(&self) -> AppResult<SourceBundleArtifact> {
        support::read_json_if_exists(&self.workspace.paths().resolve(paths::SOURCE_BUNDLE))
    }

    pub fn write_artifact_index(&self) -> AppResult<crate::models::path::WorkspaceRelPath> {
        self.journal.write_index(&self.workspace)
    }

    pub(super) fn record(
        &mut self,
        step: crate::models::artifact::ArtifactStep,
        path: &crate::models::path::WorkspaceRelPath,
        kind: crate::models::artifact::ArtifactKind,
        status: crate::models::artifact::ArtifactStatus,
        summary: &str,
    ) {
        self.journal.record(step, path, kind, status, summary);
    }

    pub(super) fn artifact_records(&self) -> &[crate::models::artifact::ArtifactRecord] {
        self.journal.records()
    }
}
