use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DependencyDiscoveryReport {
    pub constructor_candidates: Vec<DependencyCandidate>,
    pub constant_candidates: Vec<DependencyCandidate>,
    pub cast_constant_candidates: Vec<DependencyCandidate>,
    pub immutable_candidates: Vec<DependencyCandidate>,
    pub merged_candidates: Vec<DependencyCandidate>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DependencyCandidate {
    pub address: String,
    pub name: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<DependencyCandidateSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<DependencyCandidateSource>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub internal_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub solidity_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub declared_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyCandidateSource {
    Constructor,
    SourceConstant,
    SourceCastConstant,
    ImmutableConstructorAssignment,
    Unknown,
}

impl Default for DependencyCandidateSource {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DependencyDiscoveryContext {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<DependencyCandidateSource>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub internal_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub solidity_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
}
