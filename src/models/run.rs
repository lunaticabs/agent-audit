use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunTarget {
    pub address: String,
    pub chain: String,
    pub chain_id: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunRequest {
    pub address: String,
    pub chain: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: String,
    pub id_scheme: String,
    pub created_at: String,
    pub target: RunTarget,
}

impl RunRequest {
    pub fn target(&self) -> RunTarget {
        RunTarget::new(self.address.clone(), self.chain.clone())
    }
}

impl RunTarget {
    pub fn new(address: impl Into<String>, chain: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            chain: chain.into(),
            chain_id: None,
        }
    }
}
