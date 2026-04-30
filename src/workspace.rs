use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fs4::fs_std::FileExt;
use rand::Rng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::error::{AppError, AppResult};
use crate::models::identity::{ChainAlias, EvmAddress, RunId};
use crate::models::path::WorkspaceRelPath;
use crate::models::run::{RunMeta, RunRequest};
use crate::serde_ext::to_pretty_json;

#[derive(Clone, Debug)]
pub struct RunWorkspace {
    pub project_root: PathBuf,
    pub root: PathBuf,
    pub run_id: RunId,
    pub input_dir: PathBuf,
    pub artifacts_dir: PathBuf,
    pub reports_dir: PathBuf,
    pub logs_dir: PathBuf,
}

pub struct RunGuard {
    file: File,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl RunWorkspace {
    pub fn create(
        project_root: &Path,
        runs_dir: &Path,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<Self> {
        fs::create_dir_all(runs_dir)?;
        loop {
            let run_id = generate_run_id(address, chain);
            let root = runs_dir.join(run_id.as_str());
            if root.exists() {
                continue;
            }
            return Self::create_at_root(project_root, &root, &run_id, address, chain);
        }
    }

    pub fn create_at_root(
        project_root: &Path,
        root: &Path,
        run_id: &RunId,
        address: &EvmAddress,
        chain: &ChainAlias,
    ) -> AppResult<Self> {
        let workspace = Self::from_root(project_root, root, run_id);
        workspace.ensure_dirs()?;
        workspace.write_json(
            "input/run_meta.json",
            &RunMeta {
                run_id: run_id.clone(),
                id_scheme: "sha256-base64url-v1".to_string(),
                created_at: OffsetDateTime::now_utc(),
                target: RunRequest {
                    address: address.clone(),
                    chain: chain.clone(),
                }
                .target(),
            },
        )?;
        Ok(workspace)
    }

    pub fn load(project_root: &Path, runs_dir: &Path, run_id: &RunId) -> AppResult<Self> {
        let root = runs_dir.join(run_id.as_str());
        if !root.exists() {
            return Err(AppError::RunNotFound(format!(
                "run_id does not exist: {run_id}"
            )));
        }
        let workspace = Self::from_root(project_root, &root, run_id);
        workspace.ensure_dirs()?;
        Ok(workspace)
    }

    fn from_root(project_root: &Path, root: &Path, run_id: &RunId) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            root: root.to_path_buf(),
            run_id: run_id.clone(),
            input_dir: root.join("input"),
            artifacts_dir: root.join("artifacts"),
            reports_dir: root.join("reports"),
            logs_dir: root.join("logs"),
        }
    }

    pub fn ensure_dirs(&self) -> AppResult<()> {
        for dir in [
            &self.input_dir,
            &self.artifacts_dir,
            &self.reports_dir,
            &self.logs_dir,
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn write_json<T: Serialize>(
        &self,
        relative_path: impl AsRef<str>,
        payload: &T,
    ) -> AppResult<WorkspaceRelPath> {
        let relative_path = WorkspaceRelPath::new(relative_path);
        let path = self.root.join(relative_path.as_str());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&path)?;
        file.write_all(to_pretty_json(payload)?.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(self.relative(&path)?)
    }

    pub fn write_text(
        &self,
        relative_path: impl AsRef<str>,
        content: &str,
    ) -> AppResult<WorkspaceRelPath> {
        let relative_path = WorkspaceRelPath::new(relative_path);
        let path = self.root.join(relative_path.as_str());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
        Ok(self.relative(&path)?)
    }

    pub fn relative(&self, path: &Path) -> AppResult<WorkspaceRelPath> {
        let rel = path.strip_prefix(&self.root).map_err(|_| {
            AppError::Message(format!("path is outside workspace: {}", path.display()))
        })?;
        Ok(WorkspaceRelPath::new(rel.to_string_lossy()))
    }

    pub fn lock(&self) -> AppResult<RunGuard> {
        let lock_path = self.root.join(".run.lock");
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        file.lock_exclusive()?;
        Ok(RunGuard { file })
    }
}

pub fn load_request_context(workspace: &RunWorkspace) -> AppResult<RunRequest> {
    let path = workspace.root.join("input/request.json");
    if !path.exists() {
        return Err(AppError::RunNotFound(format!(
            "missing request context for run_id {}: {}",
            workspace.run_id,
            path.display()
        )));
    }
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

pub fn generate_run_id(address: &EvmAddress, chain: &ChainAlias) -> RunId {
    let created_at_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string());
    let nonce = {
        let mut bytes = [0u8; 8];
        rand::rng().fill(&mut bytes);
        hex_lower(&bytes)
    };
    let payload = format!(
        "v1|{}|{}|{}|{}",
        sanitize_token(chain.as_str()),
        sanitize_token(address.as_str()),
        created_at_ns,
        nonce
    );
    let digest = Sha256::digest(payload.as_bytes());
    let token = URL_SAFE_NO_PAD.encode(digest);
    RunId::new_unchecked(format!("v1_{token}"))
}

fn sanitize_token(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_json_persists_pretty_json_with_trailing_newline() {
        let project = TempDir::new().expect("create temp dir");
        let runs_dir = project.path().join("runs");
        let workspace = RunWorkspace::create_at_root(
            project.path(),
            &runs_dir.join("run-1"),
            &RunId::new("run-1").expect("valid run id"),
            &EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678").expect("valid address"),
            &ChainAlias::new("eth").expect("valid chain"),
        )
        .expect("create workspace");

        let relative = workspace
            .write_json(
                "input/request.json",
                &RunRequest {
                    address: EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678")
                        .expect("valid address"),
                    chain: ChainAlias::new("eth").expect("valid chain"),
                },
            )
            .expect("write request");

        assert_eq!(relative.as_str(), "input/request.json");
        let written =
            fs::read_to_string(workspace.root.join("input/request.json")).expect("read request");
        assert!(written.ends_with('\n'));
        assert!(written.contains("\n  \"address\": "));
    }
}
