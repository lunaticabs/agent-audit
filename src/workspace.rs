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
use crate::models::run::{RunMeta, RunRequest, RunTarget};
use crate::serde_ext::to_pretty_json;

pub mod paths {
    pub const RUN_META: &str = "input/run_meta.json";
    pub const REQUEST: &str = "input/request.json";
    pub const SOURCE_REQUEST: &str = "input/source_request.json";
    pub const ARTIFACT_INDEX: &str = "artifacts/artifact_index.json";
    pub const SOURCE_BUNDLE: &str = "artifacts/source_bundle.json";
    pub const SOURCE_PROVIDER_RESPONSE: &str = "artifacts/source_provider_response.json";
    pub const DEPENDENCY_FINDINGS: &str = "artifacts/dependency_findings.json";
    pub const DEPENDENCY_CHAIN_CHECKS: &str = "artifacts/dependency_chain_checks.json";
    pub const PROXY_CHECKS: &str = "artifacts/proxy_checks.json";
    pub const ORACLE_CHECKS: &str = "artifacts/oracle_checks.json";
    pub const FLASH_LOAN_SURFACE: &str = "artifacts/flash_loan_surface.json";
    pub const TOOLING_MANIFEST: &str = "artifacts/tooling_manifest.json";
    pub const MATERIALS_MANIFEST: &str = "reports/materials_manifest.json";
    pub const FINAL_REPORT: &str = "reports/final_report.json";
    pub const INIT_RUN_LOG: &str = "logs/init_run_result.json";
    pub const FETCH_SOURCE_LOG: &str = "logs/fetch_source_result.json";
    pub const RUN_DEPENDENCY_LOG: &str = "logs/run_dependency_result.json";
    pub const PREPARE_SLITHER_LOG: &str = "logs/prepare_slither_result.json";
    pub const PREPARE_TOOLING_LOG: &str = "logs/prepare_tooling_result.json";
    pub const AGGREGATE_MATERIALS_LOG: &str = "logs/aggregate_materials_result.json";
    pub const SLITHER_BUILD_MANIFEST: &str = "slither_project/build_manifest.json";
    pub const FOUNDRY_BUILD_MANIFEST: &str = "foundry_project/build_manifest.json";
    pub const ECHIDNA_BUILD_MANIFEST: &str = "echidna_project/build_manifest.json";
}

#[derive(Clone, Debug)]
pub struct RunWorkspace {
    pub project_root: PathBuf,
    paths: RunPaths,
}

pub struct RunGuard {
    file: File,
}

#[derive(Clone, Debug)]
pub struct RunPaths {
    root: PathBuf,
    run_id: RunId,
    input_dir: PathBuf,
    artifacts_dir: PathBuf,
    reports_dir: PathBuf,
    logs_dir: PathBuf,
}

#[derive(Clone, Copy, Debug)]
pub struct RunWorkspaceStore<'a> {
    root: &'a Path,
}

#[derive(Clone, Debug)]
pub struct RunLock {
    path: PathBuf,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl RunPaths {
    fn new(root: &Path, run_id: &RunId) -> Self {
        Self {
            root: root.to_path_buf(),
            run_id: run_id.clone(),
            input_dir: root.join("input"),
            artifacts_dir: root.join("artifacts"),
            reports_dir: root.join("reports"),
            logs_dir: root.join("logs"),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn input_dir(&self) -> &Path {
        &self.input_dir
    }

    pub fn artifacts_dir(&self) -> &Path {
        &self.artifacts_dir
    }

    pub fn reports_dir(&self) -> &Path {
        &self.reports_dir
    }

    pub fn logs_dir(&self) -> &Path {
        &self.logs_dir
    }

    pub fn resolve(&self, relative_path: impl AsRef<str>) -> PathBuf {
        self.root
            .join(WorkspaceRelPath::new(relative_path).as_str())
    }

    pub fn relative(&self, path: &Path) -> AppResult<WorkspaceRelPath> {
        let rel = path.strip_prefix(self.root()).map_err(|_| {
            AppError::Message(format!("path is outside workspace: {}", path.display()))
        })?;
        Ok(WorkspaceRelPath::new(rel.to_string_lossy()))
    }
}

impl RunWorkspaceStore<'_> {
    fn new(root: &Path) -> RunWorkspaceStore<'_> {
        RunWorkspaceStore { root }
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
        Ok(relative_path)
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
        Ok(relative_path)
    }
}

impl RunLock {
    fn new(root: &Path) -> Self {
        Self {
            path: root.join(".run.lock"),
        }
    }

    pub fn acquire(&self) -> AppResult<RunGuard> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.path)?;
        file.lock_exclusive()?;
        Ok(RunGuard { file })
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
        workspace.store().write_json(
            paths::RUN_META,
            &RunMeta {
                run_id: run_id.clone(),
                id_scheme: "sha256-base64url-v1".to_string(),
                created_at: OffsetDateTime::now_utc(),
                target: RunTarget::new(address.clone(), chain.clone()),
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
            paths: RunPaths::new(root, run_id),
        }
    }

    pub fn ensure_dirs(&self) -> AppResult<()> {
        for dir in [
            self.paths.input_dir(),
            self.paths.artifacts_dir(),
            self.paths.reports_dir(),
            self.paths.logs_dir(),
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        self.paths.root()
    }

    pub fn run_id(&self) -> &RunId {
        self.paths.run_id()
    }

    pub fn paths(&self) -> &RunPaths {
        &self.paths
    }

    pub fn store(&self) -> RunWorkspaceStore<'_> {
        RunWorkspaceStore::new(self.root())
    }

    pub fn lock_handle(&self) -> RunLock {
        RunLock::new(self.root())
    }
}

pub fn load_request_context(workspace: &RunWorkspace) -> AppResult<RunRequest> {
    let path = workspace.paths().resolve(paths::REQUEST);
    if !path.exists() {
        return Err(AppError::RunNotFound(format!(
            "missing request context for run_id {}: {}",
            workspace.run_id(),
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
            .store()
            .write_json(
                "input/request.json",
                &RunRequest {
                    address: EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678")
                        .expect("valid address"),
                    chain: ChainAlias::new("eth").expect("valid chain"),
                },
            )
            .expect("write request");

        assert_eq!(relative.as_str(), paths::REQUEST);
        let written =
            fs::read_to_string(workspace.paths().resolve(paths::REQUEST)).expect("read request");
        assert!(written.ends_with('\n'));
        assert!(written.contains("\n  \"address\": "));
    }
}
