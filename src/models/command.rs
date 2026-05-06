use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandName {
    InitRun,
    FetchSource,
    RunDependency,
    PrepareSlither,
    PrepareTooling,
    AggregateMaterials,
    SyncRun,
}

impl CommandName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InitRun => "init-run",
            Self::FetchSource => "fetch-source",
            Self::RunDependency => "run-dependency",
            Self::PrepareSlither => "prepare-slither",
            Self::PrepareTooling => "prepare-tooling",
            Self::AggregateMaterials => "aggregate-materials",
            Self::SyncRun => "sync-run",
        }
    }
}

impl fmt::Display for CommandName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::CommandName;

    #[test]
    fn command_name_serializes_as_kebab_case() {
        let json = serde_json::to_string(&CommandName::PrepareTooling).expect("serialize");
        assert_eq!(json, "\"prepare-tooling\"");
    }
}
