use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mongodb::bson::{self, Bson, DateTime as BsonDateTime, Document, doc};
use mongodb::options::{IndexOptions, UpdateModifications, UpdateOneModel, WriteModel};
use mongodb::sync::{Client, Collection};
use mongodb::{IndexModel, Namespace};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::AppConfig;
use crate::error::{AppResult, msg};
use crate::models::path::WorkspaceRelPath;
use crate::models::run::{RunMeta, RunRequest, RunTarget};
use crate::workspace::{RunWorkspace, paths};

const INCLUDED_TOP_LEVEL_DIRS: &[&str] = &["input", "reports", "artifacts", "sources"];
const LOCK_FILE: &str = ".run.lock";

pub struct RunSyncResult {
    pub file_count: usize,
    pub total_size_bytes: usize,
    pub upserted_file_records: usize,
}

struct SyncMaterial {
    doc: Document,
    size_bytes: usize,
}

struct RunSyncMeta {
    target: Bson,
    created_at: SystemTime,
    has_final_report: bool,
}

pub fn sync_run_to_mongo(config: &AppConfig, workspace: &RunWorkspace) -> AppResult<RunSyncResult> {
    let Some(mongo_uri) = config.mongo_uri.as_deref() else {
        return Err(msg("AGENT_AUDIT_MONGO_URI is not configured"));
    };

    let meta = load_run_sync_meta(workspace)?;
    let materials = collect_sync_materials(config, workspace)?;
    let total_size_bytes = materials.iter().map(|item| item.size_bytes).sum::<usize>();
    let file_count = materials.len();

    let client = Client::with_uri_str(mongo_uri)?;
    let db = client.database(&config.mongo_db);
    let meta_col = db.collection::<Document>(&config.mongo_runs_meta_collection);
    let files_col = db.collection::<Document>(&config.mongo_runs_files_collection);

    create_indexes(&meta_col, &files_col)?;
    upsert_file_docs(&client, config, materials)?;
    upsert_run_meta(&meta_col, workspace, meta, file_count, total_size_bytes)?;

    Ok(RunSyncResult {
        file_count,
        total_size_bytes,
        upserted_file_records: file_count,
    })
}

fn load_run_sync_meta(workspace: &RunWorkspace) -> AppResult<RunSyncMeta> {
    Ok(RunSyncMeta {
        target: read_target(workspace)?,
        created_at: read_created_at(workspace),
        has_final_report: workspace.paths().resolve(paths::FINAL_REPORT).exists(),
    })
}

fn collect_sync_materials(
    config: &AppConfig,
    workspace: &RunWorkspace,
) -> AppResult<Vec<SyncMaterial>> {
    let mut materials = Vec::new();
    for entry in walkdir::WalkDir::new(workspace.root()).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel_path = workspace.paths().relative(entry.path())?;
        if should_skip_rel_path(&rel_path) {
            continue;
        }
        materials.push(build_sync_material(
            config,
            workspace,
            entry.path(),
            rel_path,
        )?);
    }
    Ok(materials)
}

fn should_skip_rel_path(rel_path: &WorkspaceRelPath) -> bool {
    if rel_path.as_str() == LOCK_FILE {
        return true;
    }
    let Some(first_segment) = rel_path.as_str().split('/').next() else {
        return true;
    };
    !INCLUDED_TOP_LEVEL_DIRS.contains(&first_segment)
}

fn build_sync_material(
    config: &AppConfig,
    workspace: &RunWorkspace,
    full_path: &std::path::Path,
    rel_path: WorkspaceRelPath,
) -> AppResult<SyncMaterial> {
    let raw = fs::read(full_path)?;
    let size_bytes = raw.len();
    if size_bytes > config.mongo_max_inline_file_bytes {
        return Err(msg(format!(
            "file exceeds AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES: {rel_path} ({size_bytes} bytes)"
        )));
    }

    Ok(SyncMaterial {
        doc: build_file_doc(workspace, &rel_path, full_path, &raw, size_bytes)?,
        size_bytes,
    })
}

fn build_file_doc(
    workspace: &RunWorkspace,
    rel_path: &WorkspaceRelPath,
    full_path: &std::path::Path,
    raw: &[u8],
    size_bytes: usize,
) -> AppResult<Document> {
    let is_json = full_path.extension().and_then(|ext| ext.to_str()) == Some("json");
    let mut doc = Document::new();
    doc.insert("_id", format!("{}:{rel_path}", workspace.run_id()));
    doc.insert("run_id", workspace.run_id().to_string());
    doc.insert("rel_path", rel_path.as_str());
    doc.insert("size_bytes", size_bytes as i64);
    doc.insert("sha256", sha256_hex(raw));
    if is_json {
        insert_json_or_text_content(&mut doc, raw)?;
    } else {
        doc.insert("kind", "text");
        doc.insert("content_text", String::from_utf8_lossy(raw).to_string());
    }
    Ok(doc)
}

fn insert_json_or_text_content(doc: &mut Document, raw: &[u8]) -> AppResult<()> {
    match parse_json_bson(raw) {
        Ok(value) => {
            doc.insert("kind", "json");
            doc.insert("content_json", value);
        }
        Err(_) => {
            doc.insert("kind", "text");
            doc.insert("content_text", String::from_utf8_lossy(raw).to_string());
        }
    }
    Ok(())
}

fn upsert_file_docs(
    client: &Client,
    config: &AppConfig,
    materials: Vec<SyncMaterial>,
) -> AppResult<()> {
    if materials.is_empty() {
        return Ok(());
    }
    let namespace = Namespace::new(
        config.mongo_db.clone(),
        config.mongo_runs_files_collection.clone(),
    );
    let models = materials
        .into_iter()
        .map(|item| {
            let file_id = item.doc.get_str("_id").unwrap_or_default().to_string();
            WriteModel::UpdateOne(
                UpdateOneModel::builder()
                    .namespace(namespace.clone())
                    .filter(doc! {"_id": file_id})
                    .update(UpdateModifications::Document(doc! {"$set": item.doc}))
                    .upsert(true)
                    .build(),
            )
        })
        .collect::<Vec<_>>();
    client.bulk_write(models).run()?;
    Ok(())
}

fn upsert_run_meta(
    meta_col: &Collection<Document>,
    workspace: &RunWorkspace,
    meta: RunSyncMeta,
    file_count: usize,
    total_size_bytes: usize,
) -> AppResult<()> {
    let mut meta_doc = Document::new();
    meta_doc.insert("status", "succeeded");
    meta_doc.insert(
        "created_at",
        BsonDateTime::from_system_time(meta.created_at),
    );
    meta_doc.insert("target", meta.target);
    meta_doc.insert("file_count", file_count as i64);
    meta_doc.insert("total_size_bytes", total_size_bytes as i64);
    meta_doc.insert("has_final_report", meta.has_final_report);
    meta_col
        .update_one(
            doc! {"_id": workspace.run_id().to_string()},
            doc! {
                "$set": meta_doc,
                "$unset": {
                    "run_id": "",
                    "run_dir": "",
                    "materials_manifest_path": "",
                }
            },
        )
        .upsert(true)
        .run()?;
    Ok(())
}

fn create_indexes(
    meta_col: &Collection<Document>,
    files_col: &Collection<Document>,
) -> AppResult<()> {
    meta_col
        .create_index(IndexModel::builder().keys(doc! {"created_at": -1}).build())
        .run()?;
    meta_col
        .create_index(
            IndexModel::builder()
                .keys(doc! {"target.chain": 1, "target.address": 1, "created_at": -1})
                .build(),
        )
        .run()?;
    meta_col
        .create_index(
            IndexModel::builder()
                .keys(doc! {"target.address": 1, "created_at": -1})
                .build(),
        )
        .run()?;
    meta_col
        .create_index(
            IndexModel::builder()
                .keys(doc! {"status": 1, "created_at": -1})
                .build(),
        )
        .run()?;
    meta_col
        .create_index(
            IndexModel::builder()
                .keys(doc! {"has_final_report": 1, "created_at": -1})
                .build(),
        )
        .run()?;

    files_col
        .create_index(
            IndexModel::builder()
                .keys(doc! {"run_id": 1, "rel_path": 1})
                .options(IndexOptions::builder().unique(Some(true)).build())
                .build(),
        )
        .run()?;
    files_col
        .create_index(
            IndexModel::builder()
                .keys(doc! {"run_id": 1, "kind": 1})
                .build(),
        )
        .run()?;
    files_col
        .create_index(IndexModel::builder().keys(doc! {"sha256": 1}).build())
        .run()?;
    Ok(())
}

fn read_target(workspace: &RunWorkspace) -> AppResult<Bson> {
    let request_path = workspace.paths().resolve(paths::REQUEST);
    if !request_path.exists() {
        return Ok(bson::serialize_to_bson(&RunTarget::default())?);
    }
    let text = fs::read_to_string(request_path)?;
    let target = serde_json::from_str::<RunRequest>(&text)
        .map(RunRequest::into_target)
        .unwrap_or_default();
    Ok(bson::serialize_to_bson(&target)?)
}

fn read_created_at(workspace: &RunWorkspace) -> SystemTime {
    let path = workspace.paths().resolve(paths::RUN_META);
    let Ok(text) = fs::read_to_string(path) else {
        return SystemTime::now();
    };
    let Ok(payload) = serde_json::from_str::<RunMeta>(&text) else {
        return SystemTime::now();
    };
    let timestamp = payload.created_at.unix_timestamp();
    if timestamp.is_negative() {
        return SystemTime::now();
    }
    UNIX_EPOCH + Duration::from_secs(timestamp as u64)
}

fn sha256_hex(raw: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn parse_json_bson(raw: &[u8]) -> AppResult<Bson> {
    let value: Value = serde_json::from_slice(raw)?;
    Ok(bson::serialize_to_bson(&value).unwrap_or(Bson::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::identity::{ChainAlias, EvmAddress, RunId};
    use tempfile::TempDir;

    #[test]
    fn should_skip_rel_path_filters_lock_and_unknown_roots() {
        assert!(should_skip_rel_path(&WorkspaceRelPath::new(".run.lock")));
        assert!(should_skip_rel_path(&WorkspaceRelPath::new(
            "tmp/output.txt"
        )));
        assert!(!should_skip_rel_path(&WorkspaceRelPath::new(
            paths::REQUEST
        )));
        assert!(!should_skip_rel_path(&WorkspaceRelPath::new(
            paths::SOURCE_BUNDLE
        )));
    }

    #[test]
    fn build_file_doc_falls_back_to_text_for_invalid_json() {
        let temp = TempDir::new().expect("temp dir");
        let workspace = RunWorkspace::create_at_root(
            temp.path(),
            &temp.path().join("runs/run-1"),
            &RunId::new("run-1").expect("run id"),
            &EvmAddress::new("0x1234567890abcdef1234567890abcdef12345678").expect("address"),
            &ChainAlias::new("eth").expect("chain"),
        )
        .expect("workspace");

        let doc = build_file_doc(
            &workspace,
            &WorkspaceRelPath::new("artifacts/bad.json"),
            &temp.path().join("bad.json"),
            b"{not-json}",
            10,
        )
        .expect("doc");

        assert_eq!(doc.get_str("kind").expect("kind"), "text");
        assert!(
            doc.get_str("content_text")
                .expect("content")
                .contains("not-json")
        );
    }
}
