from __future__ import annotations

import base64
import hashlib
import json
import secrets
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


def _sanitize_token(value: str) -> str:
    lowered = value.lower()
    cleaned = []
    for char in lowered:
        if char.isalnum():
            cleaned.append(char)
        else:
            cleaned.append("_")
    return "".join(cleaned).strip("_")


def _run_id(address: str, chain: str) -> str:
    created_at_ns = str(time.time_ns())
    nonce = secrets.token_hex(8)
    payload = "|".join(
        [
            "v1",
            _sanitize_token(chain),
            _sanitize_token(address),
            created_at_ns,
            nonce,
        ]
    )
    digest = hashlib.sha256(payload.encode("utf-8")).digest()
    token = base64.urlsafe_b64encode(digest).decode("ascii").rstrip("=")
    return f"v1_{token}"


def generate_run_id(address: str, chain: str) -> str:
    return _run_id(address=address, chain=chain)


@dataclass
class RunWorkspace:
    project_root: Path
    root: Path
    run_id: str
    input_dir: Path
    ir_dir: Path
    artifacts_dir: Path
    reports_dir: Path
    logs_dir: Path

    @classmethod
    def create_at_root(
        cls,
        project_root: Path,
        *,
        root: Path,
        run_id: str,
        address: str,
        chain: str,
    ) -> "RunWorkspace":
        input_dir = root / "input"
        ir_dir = root / "ir"
        artifacts_dir = root / "artifacts"
        reports_dir = root / "reports"
        logs_dir = root / "logs"

        for directory in (input_dir, ir_dir, artifacts_dir, reports_dir, logs_dir):
            directory.mkdir(parents=True, exist_ok=True)

        meta_path = root / "input" / "run_meta.json"
        meta_path.write_text(
            json.dumps(
                {
                    "run_id": run_id,
                    "id_scheme": "sha256-base64url-v1",
                    "created_at": datetime.now(timezone.utc).strftime(
                        "%Y-%m-%dT%H:%M:%SZ"
                    ),
                    "target": {
                        "address": address,
                        "chain": chain,
                    },
                },
                indent=2,
                ensure_ascii=False,
            )
            + "\n"
        )

        return cls(
            project_root=project_root,
            root=root,
            run_id=run_id,
            input_dir=input_dir,
            ir_dir=ir_dir,
            artifacts_dir=artifacts_dir,
            reports_dir=reports_dir,
            logs_dir=logs_dir,
        )

    @classmethod
    def create(
        cls, project_root: Path, runs_dir: Path, address: str, chain: str
    ) -> "RunWorkspace":
        run_id = _run_id(address=address, chain=chain)
        root = runs_dir / run_id

        while root.exists():
            run_id = _run_id(address=address, chain=chain)
            root = runs_dir / run_id

        return cls.create_at_root(
            project_root=project_root,
            root=root,
            run_id=run_id,
            address=address,
            chain=chain,
        )

    @classmethod
    def load(cls, project_root: Path, runs_dir: Path, run_id: str) -> "RunWorkspace":
        root = runs_dir / run_id
        if not root.exists():
            raise FileNotFoundError(f"run_id does not exist: {run_id}")

        input_dir = root / "input"
        ir_dir = root / "ir"
        artifacts_dir = root / "artifacts"
        reports_dir = root / "reports"
        logs_dir = root / "logs"

        for directory in (input_dir, ir_dir, artifacts_dir, reports_dir, logs_dir):
            directory.mkdir(parents=True, exist_ok=True)

        return cls(
            project_root=project_root,
            root=root,
            run_id=run_id,
            input_dir=input_dir,
            ir_dir=ir_dir,
            artifacts_dir=artifacts_dir,
            reports_dir=reports_dir,
            logs_dir=logs_dir,
        )

    def write_json(self, relative_path: str, payload: Any) -> str:
        path = self.root / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n")
        return self.relative(path)

    def write_text(self, relative_path: str, content: str) -> str:
        path = self.root / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return self.relative(path)

    def read_text(self, relative_path: str) -> str:
        path = self.root / relative_path
        return path.read_text()

    def relative(self, path: Path) -> str:
        return str(path.relative_to(self.root))
