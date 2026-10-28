from __future__ import annotations

import hashlib
import json
import math
import re
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

#: Files that belong to one finished audit. Directories outside this allowlist
#: are either derived (`stats/`, `logs/`) or tooling caches (`out/`, `lib/`,
#: `cache/`, `broadcast/`) and are deliberately not sent to MongoDB.
#: `harness/` and `test/` are included because the report often cites the
#: Foundry harness as reproduction evidence. Batch-level material such as
#: `runs/_stats` or `runs/k3s-batch-*` is never read because only the single
#: run directory passed by the entrypoint is walked.
INCLUDED_TOP_LEVEL_DIRS = (
    "input",
    "reports",
    "artifacts",
    "sources",
    "harness",
    "test",
)
INCLUDED_ROOT_FILES = frozenset({"foundry.toml"})
#: Nested build/dependency trees inside an included directory. `harness/` is
#: kept (its .t.sol and src files are evidence), but its cache/out/lib trees are
#: not part of the audit record.
EXCLUDED_PATH_SEGMENTS = frozenset(
    {"cache", "out", "lib", "broadcast", "node_modules", ".git"}
)
SKIPPED_REL_PATHS = frozenset({".run.lock"})
FINAL_REPORT_REL_PATH = "reports/final_report.json"

DEFAULT_MAX_INLINE_FILE_BYTES = 8 * 1024 * 1024
DEFAULT_CHUNK_SIZE = 500
#: Keep a bulkWrite command comfortably below MongoDB's 16 MB limit. File
#: documents may each be up to max_inline_file_bytes, so count alone is not a
#: safe bound.
MAX_BULK_BYTES = 12 * 1024 * 1024
DEFAULT_TIMEOUT_MS = 5_000
DEFAULT_RETRIES = 3
MAX_SKIPPED_SAMPLES = 20

INT64_MIN = -(2**63)
INT64_MAX = 2**63 - 1

ADDRESS_RE = re.compile(r"0x[a-fA-F0-9]{40}")
CHAIN_RE = re.compile(r"\bon\s+(?:chain\s+)?([A-Za-z][A-Za-z0-9_-]*)", re.IGNORECASE)
CHAIN_STOP_WORDS = frozenset({"a", "an", "chain", "the"})

#: New `runs_meta` documents carry this marker. Historical documents written
#: by the old sync-run path have no `schema_version`; they are intentionally
#: left untouched.
META_SCHEMA_VERSION = 2


@dataclass(frozen=True)
class FileMaterial:
    """One archived workspace file."""

    doc: Dict[str, Any]
    size_bytes: int


@dataclass(frozen=True)
class MaterialSet:
    """Result of walking one run directory."""

    materials: List[FileMaterial] = field(default_factory=list)
    total_size_bytes: int = 0
    skipped_count: int = 0
    skipped_samples: List[str] = field(default_factory=list)

    @property
    def file_count(self) -> int:
        return len(self.materials)


@dataclass(frozen=True)
class PersistConfig:
    uri: str
    db: str = "agent"
    meta_collection: str = "runs_meta"
    files_collection: str = "runs_files"
    max_inline_file_bytes: int = DEFAULT_MAX_INLINE_FILE_BYTES
    timeout_ms: int = DEFAULT_TIMEOUT_MS
    chunk_size: int = DEFAULT_CHUNK_SIZE


@dataclass(frozen=True)
class PersistResult:
    run_id: str
    file_count: int
    total_size_bytes: int
    skipped_count: int
    skipped_samples: List[str]
    meta_collection: str
    files_collection: str
    duration_ms: int


@dataclass(frozen=True)
class RunPayload:
    run_id: str
    materials: List[FileMaterial]
    total_size_bytes: int
    skipped_count: int
    skipped_samples: List[str]
    meta_doc: Dict[str, Any]


# --- generic helpers --------------------------------------------------------


def utc_now() -> datetime:
    return datetime.now(timezone.utc).replace(microsecond=0)


def _as_text(value: Any) -> str:
    if isinstance(value, str):
        return value.strip()
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return str(value)
    return ""


def parse_timestamp(value: Any) -> Optional[datetime]:
    """Parse the UTC ISO-8601 timestamps written by the entrypoint."""

    text = _as_text(value)
    if not text:
        return None
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def read_json_file(path: Path) -> Tuple[Optional[Any], Optional[str]]:
    """Read a JSON file without ever raising for the persistence path."""

    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return None, "missing"
    except (OSError, UnicodeDecodeError) as exc:
        return None, "{}: {}".format(type(exc).__name__, exc)
    try:
        return json.loads(raw), None
    except ValueError as exc:
        return None, "invalid_json: {}".format(exc)


def _is_bson_safe(value: Any) -> bool:
    """Reject JSON that PyMongo/BSON cannot store without mutating it."""

    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str) or key.startswith("$") or "." in key:
                return False
            if not _is_bson_safe(item):
                return False
        return True
    if isinstance(value, (list, tuple)):
        return all(_is_bson_safe(item) for item in value)
    if isinstance(value, bool):
        return True
    if isinstance(value, int):
        return INT64_MIN <= value <= INT64_MAX
    if isinstance(value, float):
        return math.isfinite(value)
    return value is None or isinstance(value, str)


def _sanitize_bson_value(value: Any) -> Any:
    """Make the collector's metrics document safe for BSON.

    The metrics schema is generated by this repository and its keys are
    already safe. This function is a last line of defence so a single odd
    field can never make the whole run fail to persist.
    """

    if isinstance(value, dict):
        result: Dict[str, Any] = {}
        for key, item in value.items():
            safe_key = str(key)
            if safe_key.startswith("$"):
                safe_key = "_" + safe_key[1:]
            safe_key = safe_key.replace(".", "_")
            result[safe_key] = _sanitize_bson_value(item)
        return result
    if isinstance(value, (list, tuple)):
        return [_sanitize_bson_value(item) for item in value]
    if isinstance(value, bool):
        return value
    if isinstance(value, int):
        if value < INT64_MIN or value > INT64_MAX:
            return str(value)
        return value
    if isinstance(value, float) and not math.isfinite(value):
        return None
    if value is None or isinstance(value, (str, float)):
        return value
    return str(value)


def _sha256_hex(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()


def _iter_chunks(
    items: Sequence[FileMaterial],
    size: int,
    max_bytes: int = MAX_BULK_BYTES,
) -> Iterable[Sequence[FileMaterial]]:
    if size <= 0:
        size = DEFAULT_CHUNK_SIZE
    if max_bytes <= 0:
        max_bytes = MAX_BULK_BYTES

    chunk: List[FileMaterial] = []
    chunk_bytes = 0
    for item in items:
        if chunk and (len(chunk) >= size or chunk_bytes + item.size_bytes > max_bytes):
            yield chunk
            chunk = []
            chunk_bytes = 0
        chunk.append(item)
        chunk_bytes += item.size_bytes
    if chunk:
        yield chunk


# --- target and run metadata ------------------------------------------------


def target_from_prompt(prompt: str) -> Tuple[str, str]:
    """Best-effort fallback for runs that never wrote a structured report."""

    address = ""
    chain = ""
    match = ADDRESS_RE.search(prompt or "")
    if match:
        address = match.group(0)
    match = CHAIN_RE.search(prompt or "")
    if match:
        candidate = match.group(1)
        if candidate.lower() not in CHAIN_STOP_WORDS:
            chain = candidate
    return address, chain


def resolve_target(
    metrics: Any,
    report: Any,
    request: Any,
    run_meta: Any,
    prompt: str,
) -> Tuple[Optional[str], Optional[str]]:
    """Resolve the target from the most structured source available.

    ``metrics.target`` is derived from the report, so this normally returns
    exactly the audited address/chain. The prompt fallback keeps failed runs
    useful even when no report was written.
    """

    sources: List[Any] = []
    if isinstance(metrics, dict):
        sources.append(metrics.get("target"))
    if isinstance(report, dict):
        sources.append(report.get("target"))
    if isinstance(run_meta, dict):
        sources.append(run_meta.get("target"))

    address = ""
    chain = ""
    for source in sources:
        if not isinstance(source, dict):
            continue
        if not address:
            address = _as_text(source.get("address"))
        if not chain:
            chain = _as_text(source.get("chain"))

    if not address or not chain:
        prompt_address, prompt_chain = target_from_prompt(prompt)
        if not address:
            address = prompt_address
        if not chain:
            chain = prompt_chain

    return (address or None, chain or None)


def resolve_block_number(report: Any, run_meta: Any) -> Optional[str]:
    if isinstance(run_meta, dict):
        value = _as_text(run_meta.get("block_number"))
        if value:
            return value
    if not isinstance(report, dict):
        return None
    for key in ("block_number", "fork_block_number"):
        value = _as_text(report.get(key))
        if value:
            return value
    target = report.get("target")
    if isinstance(target, dict):
        value = _as_text(target.get("block_number"))
        if value:
            return value
    scope = report.get("scope")
    if isinstance(scope, dict):
        for key in ("block_number", "fork_block_number"):
            value = _as_text(scope.get(key))
            if value:
                return value
    return None


def resolve_status(run_meta: Any) -> str:
    """Map the runner's status onto the vocabulary used by historical docs."""

    if not isinstance(run_meta, dict):
        return "unknown"
    raw = _as_text(run_meta.get("status")).lower()
    exit_code = run_meta.get("exit_code")
    if raw in {"completed", "complete", "success", "succeeded", "ok"}:
        return "succeeded"
    if raw in {"failed", "failure", "error", "cancelled", "canceled", "timeout"}:
        return "failed"
    if isinstance(exit_code, int) and not isinstance(exit_code, bool):
        return "succeeded" if exit_code == 0 else "failed"
    return raw or "unknown"


def resolve_duration_seconds(
    run_meta: Any,
    started_at: Optional[datetime],
    finished_at: Optional[datetime],
) -> Optional[float]:
    if isinstance(run_meta, dict):
        raw = run_meta.get("duration_seconds")
        if isinstance(raw, (int, float)) and not isinstance(raw, bool) and raw >= 0:
            return round(float(raw), 3)
    if started_at is not None and finished_at is not None:
        seconds = (finished_at - started_at).total_seconds()
        if seconds >= 0:
            return round(seconds, 3)
    return None


# --- workspace material -----------------------------------------------------


def _relative_path(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()


def _build_file_doc(
    *,
    run_id: str,
    rel_path: str,
    path: Path,
    raw: bytes,
    created_at: Optional[datetime],
    block_number: Optional[str],
) -> Dict[str, Any]:
    doc: Dict[str, Any] = {
        "_id": "{}:{}".format(run_id, rel_path),
        "run_id": run_id,
        "rel_path": rel_path,
        "size_bytes": len(raw),
        "sha256": _sha256_hex(raw),
    }

    if path.suffix.lower() == ".json":
        try:
            text = raw.decode("utf-8")
            parsed = json.loads(text)
        except (UnicodeDecodeError, ValueError):
            doc["kind"] = "text"
            doc["content_text"] = raw.decode("utf-8", errors="replace")
        else:
            if _is_bson_safe(parsed):
                doc["kind"] = "json"
                doc["content_json"] = parsed
            else:
                # Dots/dollar keys or out-of-range integers cannot be stored
                # as a BSON object without changing the archived document.
                doc["kind"] = "text"
                doc["content_text"] = text
    else:
        doc["kind"] = "text"
        doc["content_text"] = raw.decode("utf-8", errors="replace")

    if rel_path == FINAL_REPORT_REL_PATH:
        doc["created_at"] = created_at
        doc["block_number"] = block_number
    return doc


def collect_run_materials(
    run_dir: Any,
    *,
    max_inline_file_bytes: int = DEFAULT_MAX_INLINE_FILE_BYTES,
    report_created_at: Optional[datetime] = None,
    block_number: Optional[str] = None,
) -> MaterialSet:
    """Walk exactly one run directory and build ``runs_files`` documents.

    A file larger than ``max_inline_file_bytes`` is skipped, not fatal: one
    oversized tool log must not prevent the run metadata from being archived.
    """

    run_path = Path(run_dir).resolve()
    if not run_path.is_dir():
        raise ValueError("run directory does not exist: {}".format(run_path))

    run_id = run_path.name
    materials: List[FileMaterial] = []
    total_size_bytes = 0
    skipped_count = 0
    skipped_samples: List[str] = []

    for path in sorted(run_path.rglob("*")):
        if not path.is_file():
            continue
        try:
            rel_path = _relative_path(path, run_path)
        except ValueError:
            continue
        if rel_path in SKIPPED_REL_PATHS:
            continue
        first_segment = rel_path.split("/", 1)[0]
        if (
            first_segment not in INCLUDED_TOP_LEVEL_DIRS
            and rel_path not in INCLUDED_ROOT_FILES
        ):
            continue
        if first_segment in {"harness", "test"}:
            nested_dirs = rel_path.split("/")[1:-1]
            if any(segment in EXCLUDED_PATH_SEGMENTS for segment in nested_dirs):
                continue

        try:
            resolved = path.resolve()
            if not resolved.is_relative_to(run_path):
                skipped_count += 1
                if len(skipped_samples) < MAX_SKIPPED_SAMPLES:
                    skipped_samples.append(rel_path)
                continue
            raw = path.read_bytes()
        except OSError:
            skipped_count += 1
            if len(skipped_samples) < MAX_SKIPPED_SAMPLES:
                skipped_samples.append(rel_path)
            continue

        if len(raw) > max_inline_file_bytes:
            skipped_count += 1
            if len(skipped_samples) < MAX_SKIPPED_SAMPLES:
                skipped_samples.append(rel_path)
            continue

        total_size_bytes += len(raw)
        materials.append(
            FileMaterial(
                doc=_build_file_doc(
                    run_id=run_id,
                    rel_path=rel_path,
                    path=path,
                    raw=raw,
                    created_at=report_created_at,
                    block_number=block_number,
                ),
                size_bytes=len(raw),
            )
        )

    return MaterialSet(
        materials=materials,
        total_size_bytes=total_size_bytes,
        skipped_count=skipped_count,
        skipped_samples=skipped_samples,
    )


# --- runs_meta --------------------------------------------------------------


def build_meta_document(
    run_dir: Any,
    *,
    run_meta: Any,
    request: Any,
    report: Any,
    metrics: Any,
    material_set: MaterialSet,
    stats_error: Optional[str] = None,
    now: Optional[datetime] = None,
) -> Dict[str, Any]:
    """Build the single flattened ``runs_meta`` document for one run.

    The old schema nested the target under ``target.address`` and
    ``target.chain``. New documents expose ``target_address`` and
    ``target_chain`` at the top level and embed the full metrics.json object
    under ``stats``.
    """

    run_path = Path(run_dir).resolve()
    run_id = run_path.name
    run_meta = run_meta if isinstance(run_meta, dict) else {}
    request = request if isinstance(request, dict) else {}
    report = report if isinstance(report, dict) else {}

    prompt = ""
    prompt_path = run_path / "input" / "prompt.txt"
    try:
        prompt = prompt_path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        prompt = ""

    started_at = (
        parse_timestamp(run_meta.get("started_at"))
        or parse_timestamp(run_meta.get("created_at"))
        or parse_timestamp(request.get("created_at"))
        or now
        or utc_now()
    )
    finished_at = (
        parse_timestamp(run_meta.get("finished_at"))
        or parse_timestamp(request.get("finished_at"))
        or (now or utc_now())
    )

    target_address, target_chain = resolve_target(
        metrics, report, request, run_meta, prompt
    )
    block_number = resolve_block_number(report, run_meta)

    stats_doc: Optional[Dict[str, Any]] = None
    if isinstance(metrics, dict):
        stats_doc = _sanitize_bson_value(metrics)

    doc: Dict[str, Any] = {
        "schema_version": META_SCHEMA_VERSION,
        "status": resolve_status(run_meta),
        "created_at": started_at,
        "finished_at": finished_at,
        "duration_seconds": resolve_duration_seconds(run_meta, started_at, finished_at),
        "target_address": target_address,
        "target_chain": target_chain,
        "block_number": block_number,
        "has_final_report": (run_path / FINAL_REPORT_REL_PATH).is_file(),
        "file_count": material_set.file_count,
        "total_size_bytes": material_set.total_size_bytes,
        "stats": stats_doc,
    }

    if material_set.skipped_count:
        doc["skipped_file_count"] = material_set.skipped_count
        doc["skipped_files"] = list(material_set.skipped_samples)
    if metrics is None and stats_error:
        doc["stats_error"] = stats_error

    return doc


# --- MongoDB ----------------------------------------------------------------


def _require_pymongo():
    try:
        from pymongo import MongoClient, ReplaceOne  # type: ignore
        from pymongo import ASCENDING, DESCENDING  # type: ignore
    except ImportError as exc:  # pragma: no cover - depends on environment
        raise RuntimeError(
            "pymongo is required for persistence. Install "
            "tools/run_persist/requirements.txt or use the runner image."
        ) from exc
    return MongoClient, ReplaceOne, ASCENDING, DESCENDING


def _mongo_client(config: PersistConfig):
    MongoClient, _, _, _ = _require_pymongo()
    return MongoClient(
        config.uri,
        serverSelectionTimeoutMS=config.timeout_ms,
        connectTimeoutMS=config.timeout_ms,
        socketTimeoutMS=max(config.timeout_ms, 60_000),
        appname="agent-audit-run-persist",
    )


def write_run(
    client: Any,
    config: PersistConfig,
    *,
    run_id: str,
    materials: Sequence[FileMaterial],
    meta_doc: Dict[str, Any],
) -> int:
    """Write one run: one bulk file upsert plus one metadata replace.

    The run directory is the unit of persistence. No batch-level collection is
    touched.
    """

    _, ReplaceOne, _, _ = _require_pymongo()
    db = client[config.db]
    files_collection = db[config.files_collection]
    meta_collection = db[config.meta_collection]

    written = 0
    for chunk in _iter_chunks(materials, config.chunk_size):
        operations = [
            ReplaceOne({"_id": material.doc["_id"]}, material.doc, upsert=True)
            for material in chunk
        ]
        if operations:
            files_collection.bulk_write(operations, ordered=False)
            written += len(operations)

    meta_collection.replace_one({"_id": run_id}, meta_doc, upsert=True)
    return written


def build_run_payload(
    run_dir: Any,
    *,
    max_inline_file_bytes: int = DEFAULT_MAX_INLINE_FILE_BYTES,
    now: Optional[datetime] = None,
) -> RunPayload:
    """Load one finished run and build the documents without touching MongoDB."""

    run_path = Path(run_dir).resolve()
    if not run_path.is_dir():
        raise ValueError("run directory does not exist: {}".format(run_path))
    run_id = run_path.name

    run_meta, run_meta_error = read_json_file(run_path / "input" / "run_meta.json")
    request, _ = read_json_file(run_path / "input" / "request.json")
    report, report_error = read_json_file(run_path / FINAL_REPORT_REL_PATH)
    metrics, metrics_error = read_json_file(run_path / "stats" / "metrics.json")

    if run_meta_error and run_meta_error != "missing":
        raise ValueError("input/run_meta.json: {}".format(run_meta_error))
    if report_error == "invalid_json":
        # A malformed report is still useful evidence and is archived as text.
        report = None
    stats_error = None
    if metrics_error:
        stats_error = "stats/metrics.json: {}".format(metrics_error)

    payload_now = now or utc_now()
    started_at = (
        parse_timestamp((run_meta or {}).get("started_at"))
        or parse_timestamp((run_meta or {}).get("created_at"))
        or parse_timestamp((request or {}).get("created_at"))
        or payload_now
    )
    block_number = resolve_block_number(report, run_meta)

    material_set = collect_run_materials(
        run_path,
        max_inline_file_bytes=max_inline_file_bytes,
        report_created_at=started_at,
        block_number=block_number,
    )
    meta_doc = build_meta_document(
        run_path,
        run_meta=run_meta,
        request=request,
        report=report,
        metrics=metrics,
        material_set=material_set,
        stats_error=stats_error,
        now=payload_now,
    )
    return RunPayload(
        run_id=run_id,
        materials=material_set.materials,
        total_size_bytes=material_set.total_size_bytes,
        skipped_count=material_set.skipped_count,
        skipped_samples=list(material_set.skipped_samples),
        meta_doc=meta_doc,
    )


def persist_run(run_dir: Any, config: PersistConfig) -> PersistResult:
    """Collect one finished run and persist it to MongoDB."""

    started = time.time()
    payload = build_run_payload(
        run_dir, max_inline_file_bytes=config.max_inline_file_bytes
    )

    client = _mongo_client(config)
    try:
        write_run(
            client,
            config,
            run_id=payload.run_id,
            materials=payload.materials,
            meta_doc=payload.meta_doc,
        )
    finally:
        client.close()

    return PersistResult(
        run_id=payload.run_id,
        file_count=len(payload.materials),
        total_size_bytes=payload.total_size_bytes,
        skipped_count=payload.skipped_count,
        skipped_samples=list(payload.skipped_samples),
        meta_collection=config.meta_collection,
        files_collection=config.files_collection,
        duration_ms=int((time.time() - started) * 1000),
    )


def init_indexes(config: PersistConfig) -> List[str]:
    """Create the indexes used by new flattened documents.

    This is a deployment step, not part of the per-run write. Existing indexes
    from the historical nested schema are left in place.
    """

    _, _, ASCENDING, DESCENDING = _require_pymongo()
    client = _mongo_client(config)
    try:
        db = client[config.db]
        meta_collection = db[config.meta_collection]
        files_collection = db[config.files_collection]
        names = [
            meta_collection.create_index([("created_at", DESCENDING)]),
            meta_collection.create_index(
                [
                    ("target_chain", ASCENDING),
                    ("target_address", ASCENDING),
                    ("created_at", DESCENDING),
                ]
            ),
            meta_collection.create_index(
                [("target_address", ASCENDING), ("created_at", DESCENDING)]
            ),
            meta_collection.create_index(
                [("target_chain", ASCENDING), ("created_at", DESCENDING)]
            ),
            meta_collection.create_index(
                [("status", ASCENDING), ("created_at", DESCENDING)]
            ),
            meta_collection.create_index(
                [("has_final_report", ASCENDING), ("created_at", DESCENDING)]
            ),
            files_collection.create_index(
                [("run_id", ASCENDING), ("rel_path", ASCENDING)], unique=True
            ),
            files_collection.create_index([("run_id", ASCENDING), ("kind", ASCENDING)]),
            files_collection.create_index([("sha256", ASCENDING)]),
        ]
        return names
    finally:
        client.close()
