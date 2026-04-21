from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, Dict, List

from pymongo import ASCENDING, DESCENDING, MongoClient, UpdateOne

from agent_audit.config import AppConfig
from agent_audit.workspace import RunWorkspace


INCLUDED_TOP_LEVEL_DIRS = {
    "input",
    "reports",
    "artifacts",
    "ir",
    "sources",
}


@dataclass(frozen=True)
class RunSyncResult:
    run_id: str
    file_count: int
    total_size_bytes: int
    upserted_file_records: int


def _sha256_hex(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _read_target(workspace: RunWorkspace) -> Dict[str, str]:
    request_path = workspace.root / "input" / "request.json"
    if not request_path.exists():
        return {"address": "", "chain": ""}
    try:
        payload = json.loads(request_path.read_text())
    except json.JSONDecodeError:
        return {"address": "", "chain": ""}
    if not isinstance(payload, dict):
        return {"address": "", "chain": ""}
    return {
        "address": str(payload.get("address") or ""),
        "chain": str(payload.get("chain") or ""),
    }


def sync_run_to_mongo(config: AppConfig, workspace: RunWorkspace) -> RunSyncResult:
    if not config.mongo_uri:
        raise ValueError("AGENT_AUDIT_MONGO_URI is not configured")

    target = _read_target(workspace)
    created_at = datetime.now(timezone.utc)
    run_meta_path = workspace.root / "input" / "run_meta.json"
    if run_meta_path.exists():
        try:
            run_meta_payload = json.loads(run_meta_path.read_text())
        except json.JSONDecodeError:
            run_meta_payload = {}
        if isinstance(run_meta_payload, dict):
            created_at_raw = str(run_meta_payload.get("created_at") or "")
            if created_at_raw:
                try:
                    created_at = datetime.strptime(
                        created_at_raw, "%Y-%m-%dT%H:%M:%SZ"
                    ).replace(tzinfo=timezone.utc)
                except ValueError:
                    created_at = datetime.now(timezone.utc)

    file_docs: List[Dict[str, Any]] = []
    total_size_bytes = 0

    for path in sorted(workspace.root.rglob("*")):
        if not path.is_file():
            continue
        rel_path = workspace.relative(path)
        if rel_path == ".run.lock":
            continue
        first_segment = rel_path.split("/", 1)[0]
        if first_segment not in INCLUDED_TOP_LEVEL_DIRS:
            continue

        raw = path.read_bytes()
        size_bytes = len(raw)
        if size_bytes > config.mongo_max_inline_file_bytes:
            raise ValueError(
                f"file exceeds AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES: {rel_path} ({size_bytes} bytes)"
            )

        total_size_bytes += size_bytes
        is_json = path.suffix.lower() == ".json"
        doc: Dict[str, Any] = {
            "_id": f"{workspace.run_id}:{rel_path}",
            "run_id": workspace.run_id,
            "rel_path": rel_path,
            "size_bytes": size_bytes,
            "sha256": _sha256_hex(raw),
            "kind": "json" if is_json else "text",
        }
        if is_json:
            try:
                doc["content_json"] = json.loads(raw.decode("utf-8"))
            except (json.JSONDecodeError, UnicodeDecodeError):
                doc["kind"] = "text"
                doc["content_text"] = raw.decode("utf-8", errors="replace")
        else:
            doc["content_text"] = raw.decode("utf-8", errors="replace")
        file_docs.append(doc)

    with MongoClient(config.mongo_uri) as client:
        db = client[config.mongo_db]
        meta_col = db[config.mongo_runs_meta_collection]
        files_col = db[config.mongo_runs_files_collection]

        meta_col.create_index([("created_at", DESCENDING)])
        meta_col.create_index(
            [
                ("target.chain", ASCENDING),
                ("target.address", ASCENDING),
                ("created_at", DESCENDING),
            ]
        )
        meta_col.create_index(
            [("target.address", ASCENDING), ("created_at", DESCENDING)]
        )
        meta_col.create_index([("status", ASCENDING), ("created_at", DESCENDING)])
        meta_col.create_index(
            [("has_final_report", ASCENDING), ("created_at", DESCENDING)]
        )
        files_col.create_index(
            [("run_id", ASCENDING), ("rel_path", ASCENDING)], unique=True
        )
        files_col.create_index([("run_id", ASCENDING), ("kind", ASCENDING)])
        files_col.create_index([("sha256", ASCENDING)])

        if file_docs:
            ops: List[UpdateOne] = []
            for doc in file_docs:
                selector = {"_id": doc["_id"]}
                update = {"$set": doc}
                ops.append(UpdateOne(selector, update, upsert=True))
            files_col.bulk_write(ops, ordered=False)

        meta_doc = {
            "status": "succeeded",
            "created_at": created_at,
            "target": target,
            "file_count": len(file_docs),
            "total_size_bytes": total_size_bytes,
            "has_final_report": (
                workspace.root / "reports" / "final_report.json"
            ).exists(),
        }
        meta_col.update_one(
            {"_id": workspace.run_id},
            {
                "$set": meta_doc,
                "$unset": {
                    "run_id": "",
                    "run_dir": "",
                    "materials_manifest_path": "",
                },
            },
            upsert=True,
        )

    return RunSyncResult(
        run_id=workspace.run_id,
        file_count=len(file_docs),
        total_size_bytes=total_size_bytes,
        upserted_file_records=len(file_docs),
    )
