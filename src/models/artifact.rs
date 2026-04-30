use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub step: String,
    pub path: String,
    pub kind: String,
    pub status: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtifactIndex {
    pub run_id: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub artifacts: Vec<ArtifactRecord>,
}
