use serde::{Deserialize, Serialize};

use crate::models::envelope::StepStatus;
use crate::models::path::WorkspaceRelPath;
use crate::models::run::RunTarget;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DependencyFindingsArtifact {
    pub target: RunTarget,
    pub status: StepStatus,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub findings: Vec<DependencyFinding>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DependencyFinding {
    pub title: String,
    pub severity: FindingSeverity,
    pub confidence: FindingConfidence,
    pub summary: String,
    pub source: String,
    pub location: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub evidence_artifacts: Vec<WorkspaceRelPath>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingConfidence {
    Low,
    #[default]
    Medium,
    High,
}

impl DependencyFindingsArtifact {
    pub fn new(target: RunTarget, status: StepStatus, findings: Vec<DependencyFinding>) -> Self {
        Self {
            target,
            status,
            findings,
        }
    }
}

impl DependencyFinding {
    pub fn new(
        title: impl Into<String>,
        severity: FindingSeverity,
        confidence: FindingConfidence,
        summary: impl Into<String>,
        source: impl Into<String>,
        location: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            severity,
            confidence,
            summary: summary.into(),
            source: source.into(),
            location: location.into(),
            evidence_artifacts: Vec::new(),
        }
    }

    pub fn with_evidence(mut self, evidence_artifacts: Vec<WorkspaceRelPath>) -> Self {
        self.evidence_artifacts = evidence_artifacts;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_findings_default_to_not_prepared() {
        let payload = DependencyFindingsArtifact::default();
        assert_eq!(payload.status, StepStatus::NotPrepared);
    }
}
