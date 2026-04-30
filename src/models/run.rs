use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use time::OffsetDateTime;

use crate::models::identity::{ChainAlias, ChainId, EvmAddress, RunId};

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunTarget {
    pub address: EvmAddress,
    pub chain: ChainAlias,
    pub chain_id: Option<ChainId>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunRequest {
    pub address: EvmAddress,
    pub chain: ChainAlias,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: RunId,
    pub id_scheme: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub target: RunTarget,
}

impl RunRequest {
    pub fn target(&self) -> RunTarget {
        RunTarget::new(self.address.clone(), self.chain.clone())
    }
}

impl RunTarget {
    pub fn new(address: EvmAddress, chain: ChainAlias) -> Self {
        Self {
            address,
            chain,
            chain_id: None,
        }
    }
}
