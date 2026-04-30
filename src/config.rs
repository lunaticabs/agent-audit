use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use serde_json::Value;
use url::Url;

use crate::error::{AppResult, msg};
use crate::models::identity::ChainAlias;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub project_root: PathBuf,
    pub runs_dir: PathBuf,
    pub default_chain: ChainAlias,
    pub source_api_base: Option<Url>,
    pub source_api_key: Option<String>,
    pub source_api_headers: BTreeMap<String, String>,
    pub rpc_url: Option<Url>,
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
        let runs_dir = project_root.join(runs_dir);
        Ok(Self {
            project_root,
            runs_dir,
            default_chain: env_parse_or_default(
                "AGENT_AUDIT_DEFAULT_CHAIN",
                ChainAlias::default(),
            )?,
            source_api_base: env_optional_url("AGENT_AUDIT_SOURCE_API_BASE")?,
            source_api_key: env_optional("AGENT_AUDIT_SOURCE_API_KEY"),
            source_api_headers: env_json_dict("AGENT_AUDIT_SOURCE_HEADERS_JSON")?,
            rpc_url: env_optional_url("AGENT_AUDIT_RPC_URL")?,
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

fn env_optional_url(name: &str) -> AppResult<Option<Url>> {
    let Some(value) = env_optional(name) else {
        return Ok(None);
    };
    Ok(Some(Url::parse(&value)?))
}

fn env_parse_or_default<T>(name: &str, default: T) -> AppResult<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match env_optional(name) {
        Some(value) => value
            .parse::<T>()
            .map_err(|err| msg(format!("invalid {name}: {err}"))),
        None => Ok(default),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_optional_url_rejects_invalid_url() {
        unsafe {
            env::set_var("AGENT_AUDIT_SOURCE_API_BASE", "not a url");
        }
        let error = env_optional_url("AGENT_AUDIT_SOURCE_API_BASE").expect_err("invalid url");
        assert!(error.to_string().contains("relative URL without a base"));
        unsafe {
            env::remove_var("AGENT_AUDIT_SOURCE_API_BASE");
        }
    }

    #[test]
    fn env_parse_or_default_normalizes_chain_alias() {
        unsafe {
            env::set_var("AGENT_AUDIT_DEFAULT_CHAIN", " Arbitrum-One ");
        }
        let parsed = env_parse_or_default("AGENT_AUDIT_DEFAULT_CHAIN", ChainAlias::default())
            .expect("parse chain alias");
        assert_eq!(parsed.as_str(), "arbitrumone");
        unsafe {
            env::remove_var("AGENT_AUDIT_DEFAULT_CHAIN");
        }
    }
}
