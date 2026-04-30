use serde::{Deserialize, Serialize};

use crate::models::identity::RunId;
use crate::models::path::WorkspaceRelPath;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub step: String,
    pub path: WorkspaceRelPath,
    pub kind: String,
    pub status: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactIndex {
    pub run_id: RunId,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub artifacts: Vec<ArtifactRecord>,
}
