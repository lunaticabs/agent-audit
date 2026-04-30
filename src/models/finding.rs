use serde::{Deserialize, Serialize};

use crate::models::run::RunTarget;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DependencyFindingsArtifact {
    pub target: RunTarget,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<DependencyFinding>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DependencyFinding {
    pub title: String,
    pub severity: FindingSeverity,
    pub confidence: FindingConfidence,
    pub summary: String,
    pub source: String,
    pub location: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_artifacts: Vec<String>,
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
    pub fn new(
        target: RunTarget,
        status: impl Into<String>,
        findings: Vec<DependencyFinding>,
    ) -> Self {
        Self {
            target,
            status: status.into(),
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

    pub fn with_evidence(mut self, evidence_artifacts: Vec<String>) -> Self {
        self.evidence_artifacts = evidence_artifacts;
        self
    }
}
