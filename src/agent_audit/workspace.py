from __future__ import annotations

import json
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
    def create(cls, project_root: Path, runs_dir: Path, address: str, chain: str) -> "RunWorkspace":
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        run_id = f"{stamp}_{_sanitize_token(chain)}_{_sanitize_token(address)}"
        root = runs_dir / run_id

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
