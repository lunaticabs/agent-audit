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
use time::format_description::well_known::Rfc3339;

use crate::error::{AppError, AppResult};
use crate::models::run::{RunMeta, RunRequest};
use crate::serde_ext::to_pretty_json;

#[derive(Clone, Debug)]
pub struct RunWorkspace {
    pub project_root: PathBuf,
    pub root: PathBuf,
    pub run_id: String,
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
        address: &str,
        chain: &str,
    ) -> AppResult<Self> {
        fs::create_dir_all(runs_dir)?;
        loop {
            let run_id = generate_run_id(address, chain);
            let root = runs_dir.join(&run_id);
            if root.exists() {
                continue;
            }
            return Self::create_at_root(project_root, &root, &run_id, address, chain);
        }
    }

    pub fn create_at_root(
        project_root: &Path,
        root: &Path,
        run_id: &str,
        address: &str,
        chain: &str,
    ) -> AppResult<Self> {
        let workspace = Self::from_root(project_root, root, run_id);
        workspace.ensure_dirs()?;
        workspace.write_json(
            "input/run_meta.json",
            &RunMeta {
                run_id: run_id.to_string(),
                id_scheme: "sha256-base64url-v1".to_string(),
                created_at: now_utc_rfc3339_z()?,
                target: RunRequest {
                    address: address.to_string(),
                    chain: chain.to_string(),
                }
                .target(),
            },
        )?;
        Ok(workspace)
    }

    pub fn load(project_root: &Path, runs_dir: &Path, run_id: &str) -> AppResult<Self> {
        let root = runs_dir.join(run_id);
        if !root.exists() {
            return Err(AppError::RunNotFound(format!(
                "run_id does not exist: {run_id}"
            )));
        }
        let workspace = Self::from_root(project_root, &root, run_id);
        workspace.ensure_dirs()?;
        Ok(workspace)
    }

    fn from_root(project_root: &Path, root: &Path, run_id: &str) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            root: root.to_path_buf(),
            run_id: run_id.to_string(),
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

    pub fn write_json<T: Serialize>(&self, relative_path: &str, payload: &T) -> AppResult<String> {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&path)?;
        file.write_all(to_pretty_json(payload)?.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(self.relative(&path)?)
    }

    pub fn write_text(&self, relative_path: &str, content: &str) -> AppResult<String> {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
        Ok(self.relative(&path)?)
    }

    pub fn relative(&self, path: &Path) -> AppResult<String> {
        let rel = path.strip_prefix(&self.root).map_err(|_| {
            AppError::Message(format!("path is outside workspace: {}", path.display()))
        })?;
        Ok(rel.to_string_lossy().replace('\\', "/"))
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

pub fn generate_run_id(address: &str, chain: &str) -> String {
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
        sanitize_token(chain),
        sanitize_token(address),
        created_at_ns,
        nonce
    );
    let digest = Sha256::digest(payload.as_bytes());
    let token = URL_SAFE_NO_PAD.encode(digest);
    format!("v1_{token}")
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

fn now_utc_rfc3339_z() -> AppResult<String> {
    let ts = OffsetDateTime::now_utc().format(&Rfc3339).map_err(|err| {
        AppError::Message(format!("failed to format current UTC timestamp: {err}"))
    })?;
    Ok(ts.replace("+00:00", "Z"))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
