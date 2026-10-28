from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path
from typing import Any, Dict, Optional

from . import __version__
from .persist import (
    DEFAULT_CHUNK_SIZE,
    DEFAULT_MAX_INLINE_FILE_BYTES,
    DEFAULT_RETRIES,
    DEFAULT_RPC_TIMEOUT_MS,
    DEFAULT_TIMEOUT_MS,
    PersistConfig,
    build_run_payload,
    init_indexes,
    persist_run,
)


def _env_int(name: str, default: int) -> int:
    raw = os.getenv(name, "").strip()
    if not raw:
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def _env_str(name: str, default: str) -> str:
    value = os.getenv(name, "").strip()
    return value or default


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="run_persist",
        description=(
            "Persist one finished audit run to MongoDB. The runner entrypoint "
            "calls this once after stats/metrics.json has been collected."
        ),
    )
    parser.add_argument(
        "run_dir",
        nargs="?",
        help="Finished run directory, for example /opt/agent-audit/runs/<run_id>.",
    )
    parser.add_argument(
        "--init-db",
        action="store_true",
        help="Create the runs_meta/runs_files indexes used by new documents and exit.",
    )
    parser.add_argument(
        "--uri",
        default=None,
        help="MongoDB URI. Default: AGENT_AUDIT_MONGO_URI.",
    )
    parser.add_argument(
        "--db",
        default=None,
        help="Database name. Default: AGENT_AUDIT_MONGO_DB or agent.",
    )
    parser.add_argument(
        "--collection",
        default=None,
        help=(
            "runs_meta collection. Default: AGENT_AUDIT_MONGO_RUNS_META_COLLECTION "
            "or runs_meta."
        ),
    )
    parser.add_argument(
        "--files-collection",
        default=None,
        help=(
            "runs_files collection. Default: "
            "AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION or runs_files."
        ),
    )
    parser.add_argument(
        "--max-inline-file-bytes",
        type=int,
        default=None,
        help=(
            "Skip files larger than this instead of failing the run. "
            "Default: AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES or 8388608."
        ),
    )
    parser.add_argument(
        "--timeout-ms",
        type=int,
        default=None,
        help="MongoDB server-selection timeout in milliseconds.",
    )
    parser.add_argument(
        "--rpc-url",
        default=None,
        help=(
            "RPC used to record block_number when the run did not save one. "
            "Default: AGENT_AUDIT_RPC_URL or RPC_URL."
        ),
    )
    parser.add_argument(
        "--rpc-timeout-ms",
        type=int,
        default=None,
        help="RPC request timeout in milliseconds. Default: 5000.",
    )
    parser.add_argument(
        "--retries",
        type=int,
        default=None,
        help="Attempts for a transient MongoDB failure. Default: 3.",
    )
    parser.add_argument(
        "--chunk-size",
        type=int,
        default=None,
        help="File upserts per bulk_write command. Default: 500.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Build the documents and print a summary without connecting to MongoDB.",
    )
    parser.add_argument("--version", action="version", version=__version__)
    return parser


def _config_from_args(args: argparse.Namespace) -> PersistConfig:
    uri = (args.uri or os.getenv("AGENT_AUDIT_MONGO_URI", "")).strip()
    if not uri:
        raise ValueError(
            "AGENT_AUDIT_MONGO_URI is not configured (or pass --uri); "
            "persistence is disabled"
        )

    db = args.db or _env_str("AGENT_AUDIT_MONGO_DB", "agent")
    meta_collection = args.collection or _env_str(
        "AGENT_AUDIT_MONGO_RUNS_META_COLLECTION", "runs_meta"
    )
    files_collection = args.files_collection or _env_str(
        "AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION", "runs_files"
    )
    max_inline = (
        args.max_inline_file_bytes
        if args.max_inline_file_bytes is not None
        else _env_int(
            "AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES",
            DEFAULT_MAX_INLINE_FILE_BYTES,
        )
    )
    timeout_ms = (
        args.timeout_ms
        if args.timeout_ms is not None
        else _env_int("AGENT_AUDIT_MONGO_TIMEOUT_MS", DEFAULT_TIMEOUT_MS)
    )
    chunk_size = (
        args.chunk_size
        if args.chunk_size is not None
        else _env_int("AGENT_AUDIT_MONGO_CHUNK_SIZE", DEFAULT_CHUNK_SIZE)
    )
    rpc_url = (
        args.rpc_url
        or os.getenv("AGENT_AUDIT_RPC_URL", "").strip()
        or os.getenv("RPC_URL", "").strip()
        or None
    )
    rpc_timeout_ms = (
        args.rpc_timeout_ms
        if args.rpc_timeout_ms is not None
        else _env_int("AGENT_AUDIT_RPC_TIMEOUT_MS", DEFAULT_RPC_TIMEOUT_MS)
    )

    if max_inline <= 0:
        raise ValueError("--max-inline-file-bytes must be > 0")
    if timeout_ms <= 0:
        raise ValueError("--timeout-ms must be > 0")
    if chunk_size <= 0:
        raise ValueError("--chunk-size must be > 0")
    if rpc_timeout_ms <= 0:
        raise ValueError("--rpc-timeout-ms must be > 0")

    return PersistConfig(
        uri=uri,
        db=db,
        meta_collection=meta_collection,
        files_collection=files_collection,
        max_inline_file_bytes=max_inline,
        timeout_ms=timeout_ms,
        chunk_size=chunk_size,
        rpc_url=rpc_url,
        rpc_timeout_ms=rpc_timeout_ms,
    )


def _redact_error(exc: BaseException) -> str:
    """Avoid echoing credentials if the driver includes the URI in an error."""

    text = "{}: {}".format(type(exc).__name__, exc)
    for marker in ("mongodb://", "mongodb+srv://"):
        if marker in text:
            text = "<MongoDB error redacted>"
            break
    return text


def _print_json(payload: Dict[str, Any], *, stream: Any = None) -> None:
    json.dump(payload, stream or sys.stdout, ensure_ascii=False, sort_keys=True)
    (stream or sys.stdout).write("\n")
    (stream or sys.stdout).flush()


def _run_dry(
    run_dir: str,
    max_inline_file_bytes: int,
    rpc_url: Optional[str],
    rpc_timeout_ms: int,
) -> int:
    payload = build_run_payload(
        run_dir,
        max_inline_file_bytes=max_inline_file_bytes,
        rpc_url=rpc_url,
        rpc_timeout_ms=rpc_timeout_ms,
    )
    meta = payload.meta_doc
    summary = {
        "ok": True,
        "dry_run": True,
        "run_id": payload.run_id,
        "status": meta.get("status"),
        "target_address": meta.get("target_address"),
        "target_chain": meta.get("target_chain"),
        "block_number": meta.get("block_number"),
        "block_number_source": meta.get("block_number_source"),
        "block_number_error": meta.get("block_number_error"),
        "material_count": len(meta.get("materials") or []),
        "created_at": (
            meta.get("created_at").isoformat()
            if hasattr(meta.get("created_at"), "isoformat")
            else meta.get("created_at")
        ),
        "file_count": len(payload.materials),
        "total_size_bytes": payload.total_size_bytes,
        "skipped_file_count": payload.skipped_count,
        "skipped_files": payload.skipped_samples,
        "has_stats": isinstance(meta.get("stats"), dict),
        "stats_schema_version": (
            meta.get("stats", {}).get("stats_schema_version")
            if isinstance(meta.get("stats"), dict)
            else None
        ),
    }
    _print_json(summary)
    return 0


def _run_persist(run_dir: str, config: PersistConfig, retries: int) -> int:
    last_error: Optional[BaseException] = None
    for attempt in range(1, max(1, retries) + 1):
        try:
            result = persist_run(run_dir, config)
        except Exception as exc:  # noqa: BLE001 - CLI boundary
            last_error = exc
            if attempt >= max(1, retries):
                break
            time.sleep(min(2 ** (attempt - 1), 5))
            continue

        _print_json(
            {
                "ok": True,
                "run_id": result.run_id,
                "db": config.db,
                "meta_collection": result.meta_collection,
                "files_collection": result.files_collection,
                "file_count": result.file_count,
                "total_size_bytes": result.total_size_bytes,
                "skipped_file_count": result.skipped_count,
                "skipped_files": result.skipped_samples,
                "duration_ms": result.duration_ms,
                "attempts": attempt,
            }
        )
        return 0

    message = _redact_error(last_error) if last_error else "unknown error"
    _print_json(
        {
            "ok": False,
            "run_id": Path(run_dir).resolve().name,
            "error": message,
        },
        stream=sys.stderr,
    )
    return 1


def main(argv: Optional[list[str]] = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if args.init_db:
        try:
            config = _config_from_args(args)
            names = init_indexes(config)
        except Exception as exc:  # noqa: BLE001 - CLI boundary
            _print_json(
                {"ok": False, "action": "init-db", "error": _redact_error(exc)},
                stream=sys.stderr,
            )
            return 1
        _print_json(
            {
                "ok": True,
                "action": "init-db",
                "db": config.db,
                "meta_collection": config.meta_collection,
                "files_collection": config.files_collection,
                "indexes": names,
            }
        )
        return 0

    if not args.run_dir:
        parser.error("run_dir is required unless --init-db is used")

    run_dir = str(Path(args.run_dir).resolve())
    if not Path(run_dir).is_dir():
        _print_json(
            {"ok": False, "run_dir": run_dir, "error": "run directory does not exist"},
            stream=sys.stderr,
        )
        return 1

    if args.dry_run:
        max_inline = (
            args.max_inline_file_bytes
            if args.max_inline_file_bytes is not None
            else _env_int(
                "AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES",
                DEFAULT_MAX_INLINE_FILE_BYTES,
            )
        )
        rpc_url = (
            args.rpc_url
            or os.getenv("AGENT_AUDIT_RPC_URL", "").strip()
            or os.getenv("RPC_URL", "").strip()
            or None
        )
        rpc_timeout_ms = (
            args.rpc_timeout_ms
            if args.rpc_timeout_ms is not None
            else _env_int("AGENT_AUDIT_RPC_TIMEOUT_MS", DEFAULT_RPC_TIMEOUT_MS)
        )
        try:
            return _run_dry(run_dir, max_inline, rpc_url, rpc_timeout_ms)
        except Exception as exc:  # noqa: BLE001 - CLI boundary
            _print_json(
                {"ok": False, "dry_run": True, "error": _redact_error(exc)},
                stream=sys.stderr,
            )
            return 1

    try:
        config = _config_from_args(args)
    except ValueError as exc:
        _print_json(
            {"ok": False, "error": str(exc)},
            stream=sys.stderr,
        )
        return 2

    retries = (
        args.retries
        if args.retries is not None
        else _env_int("AGENT_AUDIT_MONGO_RETRIES", DEFAULT_RETRIES)
    )
    if retries < 1:
        retries = 1
    return _run_persist(run_dir, config, retries)


if __name__ == "__main__":
    raise SystemExit(main())
