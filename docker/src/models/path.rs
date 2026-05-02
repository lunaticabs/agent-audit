use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelativePath(String);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceRelPath(String);

impl RelativePath {
    pub fn new(value: impl AsRef<str>) -> Self {
        Self(normalize_relative_path(value.as_ref(), true))
    }

    pub fn dot() -> Self {
        Self(".".to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn join(&self, child: impl AsRef<str>) -> Self {
        if self.is_dot() {
            Self::new(child)
        } else {
            Self::new(format!("{}/{}", self.0, child.as_ref()))
        }
    }

    pub fn parent(&self) -> Option<Self> {
        if self.is_dot() {
            return None;
        }
        let parent = Path::new(&self.0).parent()?;
        let rendered = normalize_relative_path(&parent.to_string_lossy(), true);
        if rendered == "." {
            None
        } else {
            Some(Self(rendered))
        }
    }

    pub fn is_dot(&self) -> bool {
        self.0 == "."
    }
}

impl Default for RelativePath {
    fn default() -> Self {
        Self::dot()
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RelativePath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for RelativePath {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for RelativePath {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl WorkspaceRelPath {
    pub fn new(value: impl AsRef<str>) -> Self {
        Self(normalize_relative_path(value.as_ref(), false))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for WorkspaceRelPath {
    fn default() -> Self {
        Self(".".to_string())
    }
}

impl fmt::Display for WorkspaceRelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for WorkspaceRelPath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for WorkspaceRelPath {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for WorkspaceRelPath {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<WorkspaceRelPath> for RelativePath {
    fn from(value: WorkspaceRelPath) -> Self {
        Self(value.0)
    }
}

fn normalize_relative_path(value: &str, allow_parent_segments: bool) -> String {
    let raw = value.trim().replace('\\', "/");
    if raw.is_empty() || raw == "." {
        return ".".to_string();
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." if allow_parent_segments => {
                if let Some(last) = parts.last().copied() {
                    if last != ".." {
                        parts.pop();
                    } else {
                        parts.push("..");
                    }
                } else {
                    parts.push("..");
                }
            }
            ".." => {}
            _ => parts.push(part),
        }
    }

    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_normalizes_current_dir_segments() {
        let path = RelativePath::new("./contracts//A.sol");
        assert_eq!(path.as_str(), "contracts/A.sol");
    }

    #[test]
    fn relative_path_preserves_parent_segments() {
        let path = RelativePath::new("../contracts/./A.sol");
        assert_eq!(path.as_str(), "../contracts/A.sol");
    }

    #[test]
    fn workspace_rel_path_strips_parent_segments() {
        let path = WorkspaceRelPath::new("../artifacts/./result.json");
        assert_eq!(path.as_str(), "artifacts/result.json");
    }
}
