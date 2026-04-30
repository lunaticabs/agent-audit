use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::error::AppResult;
use crate::models::artifact::ArtifactRecord;
use crate::models::source::SourceFile;
use crate::workspace::RunWorkspace;

use super::AuditPipelineService;

pub(super) fn recreate_dir(path: &Path) -> AppResult<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    fs::create_dir_all(path)?;
    Ok(())
}

pub(super) fn recreate_symlink(link_path: &Path, target_path: &Path) -> AppResult<()> {
    if link_path.exists() || link_path.symlink_metadata().is_ok() {
        let metadata = link_path.symlink_metadata()?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(link_path)?;
        } else {
            fs::remove_file(link_path)?;
        }
    }
    if let Some(parent) = link_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let relative_target = pathdiff::diff_paths(
        target_path,
        link_path.parent().unwrap_or_else(|| Path::new(".")),
    )
    .unwrap_or_else(|| target_path.to_path_buf());
    #[cfg(unix)]
    std::os::unix::fs::symlink(relative_target, link_path)?;
    #[cfg(windows)]
    {
        if target_path.is_dir() {
            std::os::windows::fs::symlink_dir(relative_target, link_path)?;
        } else {
            std::os::windows::fs::symlink_file(relative_target, link_path)?;
        }
    }
    Ok(())
}

pub(super) fn load_existing_artifacts(workspace: &RunWorkspace) -> Vec<ArtifactRecord> {
    let path = workspace.root.join("artifacts/artifact_index.json");
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<crate::models::artifact::ArtifactIndex>(&text)
        .map(|payload| payload.artifacts)
        .unwrap_or_default()
}

pub(super) fn read_json_if_exists<T>(path: &Path) -> AppResult<T>
where
    T: Default + serde::de::DeserializeOwned,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

pub(super) fn path_parent_string(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|parent| {
            let rendered = parent.to_string_lossy().replace('\\', "/");
            if rendered == "." {
                String::new()
            } else {
                rendered
            }
        })
        .unwrap_or_default()
}

pub(super) fn render_line_list(items: &[String]) -> String {
    if items.is_empty() {
        String::new()
    } else {
        format!("{}\n", items.join("\n"))
    }
}

pub(super) fn format_path_for_json(path: &Path) -> String {
    let rendered = path.to_string_lossy().replace('\\', "/");
    if rendered.is_empty() || rendered == "." {
        ".".to_string()
    } else {
        rendered
    }
}

impl AuditPipelineService {
    pub(super) fn existing_paths(&self, relative_paths: &[&str]) -> Vec<String> {
        relative_paths
            .iter()
            .filter(|path| self.workspace.root.join(path).exists())
            .map(|path| (*path).to_string())
            .collect()
    }

    pub(super) fn existing_tree(&self, relative_roots: &[&str]) -> AppResult<Vec<String>> {
        let mut existing = Vec::new();
        let mut seen = BTreeSet::new();
        for root in relative_roots {
            let path = self.workspace.root.join(root);
            if path.is_file() {
                if seen.insert((*root).to_string()) {
                    existing.push((*root).to_string());
                }
                continue;
            }
            if !path.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&path).sort_by_file_name() {
                let entry = entry?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let relative = self.workspace.relative(entry.path())?;
                if seen.insert(relative.clone()) {
                    existing.push(relative);
                }
            }
        }
        Ok(existing)
    }

    pub(super) fn write_source_text(
        &mut self,
        source_file: &SourceFile,
        final_path: &str,
        summary_prefix: &str,
    ) -> AppResult<()> {
        self.workspace
            .write_text(&format!("sources/{final_path}"), &source_file.content)?;
        self.record(
            "fetch_contract_source",
            &format!("sources/{final_path}"),
            "source",
            "executed",
            summary_prefix,
        );
        Ok(())
    }
}
