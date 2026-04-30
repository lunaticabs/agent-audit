use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{AppResult, msg};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub project_root: PathBuf,
    pub runs_dir: PathBuf,
    pub default_chain: String,
    pub source_api_base: Option<String>,
    pub source_api_key: Option<String>,
    pub source_api_headers: BTreeMap<String, String>,
    pub rpc_url: Option<String>,
    pub mongo_uri: Option<String>,
    pub mongo_db: String,
    pub mongo_runs_meta_collection: String,
    pub mongo_runs_files_collection: String,
    pub mongo_max_inline_file_bytes: usize,
}

impl AppConfig {
    pub fn load(project_root: Option<PathBuf>) -> AppResult<Self> {
        let project_root = project_root.unwrap_or_else(default_project_root);
        let env_path = project_root.join(".env");
        let _ = dotenvy::from_path(&env_path);

        let runs_dir = env::var("AGENT_AUDIT_RUNS_DIR").unwrap_or_else(|_| "runs".to_string());
        Ok(Self {
            project_root: project_root.clone(),
            runs_dir: project_root.join(runs_dir),
            default_chain: env::var("AGENT_AUDIT_DEFAULT_CHAIN")
                .unwrap_or_else(|_| "eth".to_string()),
            source_api_base: env_optional("AGENT_AUDIT_SOURCE_API_BASE"),
            source_api_key: env_optional("AGENT_AUDIT_SOURCE_API_KEY"),
            source_api_headers: env_json_dict("AGENT_AUDIT_SOURCE_HEADERS_JSON")?,
            rpc_url: env_optional("AGENT_AUDIT_RPC_URL"),
            mongo_uri: env_optional("AGENT_AUDIT_MONGO_URI"),
            mongo_db: env::var("AGENT_AUDIT_MONGO_DB")
                .unwrap_or_else(|_| "agent_audit".to_string()),
            mongo_runs_meta_collection: env::var("AGENT_AUDIT_MONGO_RUNS_META_COLLECTION")
                .unwrap_or_else(|_| "runs_meta".to_string()),
            mongo_runs_files_collection: env::var("AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION")
                .unwrap_or_else(|_| "runs_files".to_string()),
            mongo_max_inline_file_bytes: env::var("AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(8 * 1024 * 1024),
        })
    }
}

fn default_project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn env_optional(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn env_json_dict(name: &str) -> AppResult<BTreeMap<String, String>> {
    let Some(raw) = env_optional(name) else {
        return Ok(BTreeMap::new());
    };
    let value: Value = serde_json::from_str(&raw)?;
    let object = value
        .as_object()
        .ok_or_else(|| msg(format!("{name} must be a JSON object")))?;
    let mut result = BTreeMap::new();
    for (key, value) in object {
        result.insert(
            key.clone(),
            value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| value.to_string()),
        );
    }
    Ok(result)
}
