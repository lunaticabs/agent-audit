use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DependencyDiscoveryReport {
    pub constructor_candidates: Vec<DependencyCandidate>,
    pub constant_candidates: Vec<DependencyCandidate>,
    pub cast_constant_candidates: Vec<DependencyCandidate>,
    pub immutable_candidates: Vec<DependencyCandidate>,
    pub merged_candidates: Vec<DependencyCandidate>,
}

#[skip_serializing_none]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DependencyCandidate {
    pub address: String,
    pub name: String,
    pub role: String,
    pub source: Option<DependencyCandidateSource>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub sources: Vec<DependencyCandidateSource>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub internal_type: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub solidity_type: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub declared_type: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
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
#[serde(default)]
pub struct DependencyDiscoveryContext {
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub sources: Vec<DependencyCandidateSource>,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub internal_type: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub solidity_type: String,
    #[serde(skip_serializing_if = "crate::serde_ext::is_empty")]
    pub file: String,
}
