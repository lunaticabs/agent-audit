from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, Optional

try:
    from dotenv import load_dotenv
except ImportError:  # pragma: no cover - optional during bootstrap
    load_dotenv = None


def _env_json_dict(name: str) -> Dict[str, str]:
    raw = os.getenv(name, "").strip()
    if not raw:
        return {}
    parsed = json.loads(raw)
    if not isinstance(parsed, dict):
        raise ValueError(f"{name} must be a JSON object")
    return {str(key): str(value) for key, value in parsed.items()}


@dataclass(frozen=True)
class AppConfig:
    project_root: Path
    runs_dir: Path
    default_chain: str
    source_api_base: Optional[str]
    source_api_key: Optional[str]
    source_api_headers: Dict[str, str]
    rpc_url: Optional[str]
    mongo_uri: Optional[str]
    mongo_db: str
    mongo_runs_meta_collection: str
    mongo_runs_files_collection: str
    mongo_max_inline_file_bytes: int

    @classmethod
    def load(cls, project_root: Optional[Path] = None) -> "AppConfig":
        if project_root is None:
            project_root = Path(__file__).resolve().parents[2]

        if load_dotenv is not None:
            load_dotenv(project_root / ".env")

        runs_dir = os.getenv("AGENT_AUDIT_RUNS_DIR", "runs")

        return cls(
            project_root=project_root,
            runs_dir=(project_root / runs_dir).resolve(),
            default_chain=os.getenv("AGENT_AUDIT_DEFAULT_CHAIN", "eth"),
            source_api_base=os.getenv("AGENT_AUDIT_SOURCE_API_BASE") or None,
            source_api_key=os.getenv("AGENT_AUDIT_SOURCE_API_KEY") or None,
            source_api_headers=_env_json_dict("AGENT_AUDIT_SOURCE_HEADERS_JSON"),
            rpc_url=os.getenv("AGENT_AUDIT_RPC_URL") or None,
            mongo_uri=os.getenv("AGENT_AUDIT_MONGO_URI") or None,
            mongo_db=os.getenv("AGENT_AUDIT_MONGO_DB", "agent_audit"),
            mongo_runs_meta_collection=os.getenv(
                "AGENT_AUDIT_MONGO_RUNS_META_COLLECTION", "runs_meta"
            ),
            mongo_runs_files_collection=os.getenv(
                "AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION", "runs_files"
            ),
            mongo_max_inline_file_bytes=int(
                os.getenv(
                    "AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES", str(8 * 1024 * 1024)
                )
            ),
        )
