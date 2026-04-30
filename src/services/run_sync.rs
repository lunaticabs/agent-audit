use std::fs;

use mongodb::IndexModel;
use mongodb::Namespace;
use mongodb::bson::{self, Bson, DateTime as BsonDateTime, Document, doc};
use mongodb::options::{IndexOptions, UpdateModifications, UpdateOneModel, WriteModel};
use mongodb::sync::{Client, Collection};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::AppConfig;
use crate::error::{AppResult, msg};
use crate::models::identity::RunId;
use crate::models::run::{RunMeta, RunRequest, RunTarget};
use crate::workspace::RunWorkspace;

const INCLUDED_TOP_LEVEL_DIRS: &[&str] = &["input", "reports", "artifacts", "sources"];

pub struct RunSyncResult {
    pub run_id: RunId,
    pub file_count: usize,
    pub total_size_bytes: usize,
    pub upserted_file_records: usize,
}

pub fn sync_run_to_mongo(config: &AppConfig, workspace: &RunWorkspace) -> AppResult<RunSyncResult> {
    let Some(mongo_uri) = config.mongo_uri.as_deref() else {
        return Err(msg("AGENT_AUDIT_MONGO_URI is not configured"));
    };
    let target = read_target(workspace)?;
    let created_at = read_created_at(workspace);
    let mut file_docs = Vec::new();
    let mut total_size_bytes = 0usize;

    for entry in walkdir::WalkDir::new(&workspace.root).sort_by_file_name() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel_path = workspace.relative(entry.path())?;
        if rel_path.as_str() == ".run.lock" {
            continue;
        }
        let Some(first_segment) = rel_path.as_str().split('/').next() else {
            continue;
        };
        if !INCLUDED_TOP_LEVEL_DIRS.contains(&first_segment) {
            continue;
        }
        let raw = fs::read(entry.path())?;
        let size_bytes = raw.len();
        if size_bytes > config.mongo_max_inline_file_bytes {
            return Err(msg(format!(
                "file exceeds AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES: {rel_path} ({size_bytes} bytes)"
            )));
        }
        total_size_bytes += size_bytes;
        let is_json = entry.path().extension().and_then(|ext| ext.to_str()) == Some("json");
        let mut doc = Document::new();
        doc.insert("_id", format!("{}:{rel_path}", workspace.run_id));
        doc.insert("run_id", workspace.run_id.to_string());
        doc.insert("rel_path", rel_path.as_str());
        doc.insert("size_bytes", size_bytes as i64);
        doc.insert("sha256", sha256_hex(&raw));
        doc.insert("kind", if is_json { "json" } else { "text" });
        if is_json {
            match parse_json_bson(&raw) {
                Ok(value) => {
                    doc.insert("content_json", value);
                }
                Err(_) => {
                    doc.insert("kind", "text");
                    doc.insert("content_text", String::from_utf8_lossy(&raw).to_string());
                }
            }
        } else {
            doc.insert("content_text", String::from_utf8_lossy(&raw).to_string());
        }
        file_docs.push(doc);
    }

    let client = Client::with_uri_str(mongo_uri)?;
    let db = client.database(&config.mongo_db);
    let meta_col = db.collection::<Document>(&config.mongo_runs_meta_collection);
    let files_col = db.collection::<Document>(&config.mongo_runs_files_collection);

    create_indexes(&meta_col, &files_col)?;

    if !file_docs.is_empty() {
        let namespace = Namespace::new(
            config.mongo_db.clone(),
            config.mongo_runs_files_collection.clone(),
        );
        let models = file_docs
            .iter()
            .map(|file_doc| {
                WriteModel::UpdateOne(
                    UpdateOneModel::builder()
                        .namespace(namespace.clone())
                        .filter(doc! {"_id": file_doc.get_str("_id").unwrap_or_default()})
                        .update(UpdateModifications::Document(
                            doc! {"$set": file_doc.clone()},
                        ))
                        .upsert(true)
                        .build(),
                )
            })
            .collect::<Vec<_>>();
        client.bulk_write(models).run()?;
    }

    let mut meta_doc = Document::new();
    meta_doc.insert("status", "succeeded");
    meta_doc.insert("created_at", BsonDateTime::from_system_time(created_at));
    meta_doc.insert("target", target);
    meta_doc.insert("file_count", file_docs.len() as i64);
    meta_doc.insert("total_size_bytes", total_size_bytes as i64);
    meta_doc.insert(
        "has_final_report",
        workspace.root.join("reports/final_report.json").exists(),
    );
    meta_col
        .update_one(
            doc! {"_id": workspace.run_id.to_string()},
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

    Ok(RunSyncResult {
        run_id: workspace.run_id.clone(),
        file_count: file_docs.len(),
        total_size_bytes,
        upserted_file_records: file_docs.len(),
    })
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
    let request_path = workspace.root.join("input/request.json");
    if !request_path.exists() {
        return Ok(bson::serialize_to_bson(&RunTarget::default())?);
    }
    let text = fs::read_to_string(request_path)?;
    let payload = serde_json::from_str::<RunRequest>(&text).unwrap_or_default();
    Ok(bson::serialize_to_bson(&payload.target())?)
}

fn read_created_at(workspace: &RunWorkspace) -> std::time::SystemTime {
    let path = workspace.root.join("input/run_meta.json");
    let Ok(text) = fs::read_to_string(path) else {
        return std::time::SystemTime::now();
    };
    let Ok(payload) = serde_json::from_str::<RunMeta>(&text) else {
        return std::time::SystemTime::now();
    };
    let timestamp = payload.created_at.unix_timestamp();
    if timestamp.is_negative() {
        return std::time::SystemTime::now();
    }
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(timestamp as u64)
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
