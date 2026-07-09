use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use time::OffsetDateTime;

use crate::error::{AppError, msg};
use crate::models::identity::{ChainAlias, ChainId, EvmAddress, RunId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    #[default]
    OpenSource,
    ClosedSource,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunTarget {
    pub address: EvmAddress,
    pub chain: ChainAlias,
    pub chain_id: Option<ChainId>,
    pub source_kind: SourceKind,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunRequest {
    pub address: EvmAddress,
    pub chain: ChainAlias,
    pub source_kind: SourceKind,
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
    pub fn into_target(self) -> RunTarget {
        self.into()
    }
}

impl RunTarget {
    pub fn new(address: EvmAddress, chain: ChainAlias) -> Self {
        Self::new_with_source_kind(address, chain, SourceKind::OpenSource)
    }

    pub fn new_with_source_kind(
        address: EvmAddress,
        chain: ChainAlias,
        source_kind: SourceKind,
    ) -> Self {
        Self {
            address,
            chain,
            chain_id: None,
            source_kind,
        }
    }
}

impl From<RunRequest> for RunTarget {
    fn from(value: RunRequest) -> Self {
        Self::new_with_source_kind(value.address, value.chain, value.source_kind)
    }
}

impl SourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenSource => "open_source",
            Self::ClosedSource => "closed_source",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SourceKind {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "open_source" => Ok(Self::OpenSource),
            "closed_source" => Ok(Self::ClosedSource),
            _ => Err(msg(format!(
                "source_kind must be open-source or closed-source, got {value:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SourceKind;

    #[test]
    fn source_kind_accepts_cli_and_json_spellings() {
        assert_eq!(
            "open-source".parse::<SourceKind>().expect("parse"),
            SourceKind::OpenSource
        );
        assert_eq!(
            "closed_source".parse::<SourceKind>().expect("parse"),
            SourceKind::ClosedSource
        );
    }

    #[test]
    fn source_kind_serializes_as_snake_case() {
        let json = serde_json::to_string(&SourceKind::ClosedSource).expect("serialize");
        assert_eq!(json, "\"closed_source\"");
    }
}
