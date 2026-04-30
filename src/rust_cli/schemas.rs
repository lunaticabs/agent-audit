use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub step: String,
    pub path: String,
    pub kind: String,
    pub status: String,
    pub summary: String,
}
