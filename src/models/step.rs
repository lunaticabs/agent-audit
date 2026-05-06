use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    NotPrepared,
    Prepared,
    Executed,
    ConfiguredNotExecuted,
    ExecutedWithError,
    SourceFetched,
    SourceFetchFailed,
    SourceNotFetched,
    SourceFilesMissing,
    SourceApiNotConfigured,
}

impl StepStatus {
    pub const fn is_precondition_failure(self) -> bool {
        matches!(
            self,
            Self::SourceNotFetched | Self::SourceFilesMissing | Self::ConfiguredNotExecuted
        )
    }

    pub const fn is_retryable_failure(self) -> bool {
        matches!(self, Self::SourceFetchFailed | Self::ExecutedWithError)
    }
}

impl FromStr for StepStatus {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "not_prepared" => Ok(Self::NotPrepared),
            "prepared" => Ok(Self::Prepared),
            "executed" => Ok(Self::Executed),
            "configured_not_executed" => Ok(Self::ConfiguredNotExecuted),
            "executed_with_error" => Ok(Self::ExecutedWithError),
            "source_fetched" => Ok(Self::SourceFetched),
            "source_fetch_failed" => Ok(Self::SourceFetchFailed),
            "source_not_fetched" => Ok(Self::SourceNotFetched),
            "source_files_missing" => Ok(Self::SourceFilesMissing),
            "source_api_not_configured" => Ok(Self::SourceApiNotConfigured),
            _ => Err("unknown step status"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StepStatus;

    #[test]
    fn step_status_serializes_as_snake_case() {
        let json = serde_json::to_string(&StepStatus::SourceApiNotConfigured)
            .expect("serialize step status");
        assert_eq!(json, "\"source_api_not_configured\"");
    }

    #[test]
    fn step_status_parses_artifact_only_variants() {
        assert_eq!(
            "configured_not_executed"
                .parse::<StepStatus>()
                .expect("parse status"),
            StepStatus::ConfiguredNotExecuted
        );
        assert_eq!(
            "executed_with_error"
                .parse::<StepStatus>()
                .expect("parse status"),
            StepStatus::ExecutedWithError
        );
    }
}
