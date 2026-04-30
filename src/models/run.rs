use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunTarget {
    pub address: String,
    pub chain: String,
    #[serde(skip_serializing_if = "Option::is_none")]
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
