from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
import fcntl
import json
import os
import shlex
import socket
import subprocess
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Sequence

from agent_audit.config import AppConfig
from agent_audit.orchestrator.run import load_request_context, load_workspace
from agent_audit.services.pipeline import AuditPipelineService
from agent_audit.tooling import prefixed_command, run_command
from agent_audit.workspace import RunWorkspace


def utc_timestamp() -> str:
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def slugify(value: str) -> str:
    lowered = value.lower()
    cleaned: List[str] = []
    for char in lowered:
        if char.isalnum():
            cleaned.append(char)
        else:
            cleaned.append("_")
    return "".join(cleaned).strip("_")


def read_json_if_exists(path: Path, default: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
    fallback = dict(default or {})
    if not path.exists():
        return fallback
    try:
        payload = json.loads(path.read_text())
    except json.JSONDecodeError:
        return fallback
    return payload if isinstance(payload, dict) else fallback


def load_workspace_and_context(
    config: AppConfig,
    run_id: str,
) -> tuple[RunWorkspace, Dict[str, str]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    return workspace, {"address": context.address, "chain": context.chain}


@contextmanager
def run_lock(workspace: RunWorkspace) -> Iterator[None]:
    lock_path = workspace.root / ".run.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+") as handle:
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


@dataclass(frozen=True)
class InvocationPaths:
    relative_dir: str
    absolute_dir: Path
    plan_path: str
    stdout_path: str
    stderr_path: str
    result_path: str


def create_invocation_paths(
    workspace: RunWorkspace,
    *,
    category: str,
    tool_name: str,
    label: str,
) -> InvocationPaths:
    stem = f"{utc_timestamp()}_{slugify(label) or slugify(tool_name) or 'run'}"
    relative_dir = f"artifacts/{category}/{tool_name}/{stem}"
    absolute_dir = workspace.root / relative_dir
    absolute_dir.mkdir(parents=True, exist_ok=True)
    return InvocationPaths(
        relative_dir=relative_dir,
        absolute_dir=absolute_dir,
        plan_path=f"{relative_dir}/plan.json",
        stdout_path=f"{relative_dir}/stdout.txt",
        stderr_path=f"{relative_dir}/stderr.txt",
        result_path=f"{relative_dir}/result.json",
    )


def write_json_file(path: Path, payload: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n")


def append_index_entry(
    workspace: RunWorkspace,
    *,
    relative_index_path: str,
    entry: Dict[str, Any],
) -> str:
    index_path = workspace.root / relative_index_path
    payload = read_json_if_exists(index_path, {"run_id": workspace.run_id, "entries": []})
    entries = payload.get("entries")
    if not isinstance(entries, list):
        entries = []
    entries.append(entry)
    payload["run_id"] = workspace.run_id
    payload["entries"] = entries
    write_json_file(index_path, payload)
    return relative_index_path


def refresh_materials_manifest(config: AppConfig, workspace: RunWorkspace) -> str:
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    return pipeline.aggregate_materials(address=context.address, chain=context.chain)


def shell_join(parts: Sequence[str]) -> str:
    return shlex.join(list(parts))


def find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def resolve_rpc_url(config: AppConfig, explicit_rpc_url: Optional[str]) -> str:
    if explicit_rpc_url:
        return explicit_rpc_url
    if config.rpc_url:
        return config.rpc_url
    raise ValueError("rpc_url is required because AGENT_AUDIT_RPC_URL is not configured")


def success_payload(
    *,
    tool: str,
    run_id: str,
    status: str,
    summary: str,
    artifacts: List[str],
    extra: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    payload: Dict[str, Any] = {
        "ok": status not in {"failed", "error"},
        "tool": tool,
        "run_id": run_id,
        "status": status,
        "summary": summary,
        "artifacts": artifacts,
    }
    if extra:
        payload.update(extra)
    return payload


def error_payload(
    *,
    tool: str,
    summary: str,
    run_id: Optional[str] = None,
    artifacts: Optional[List[str]] = None,
    extra: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    payload: Dict[str, Any] = {
        "ok": False,
        "tool": tool,
        "status": "error",
        "summary": summary,
        "artifacts": artifacts or [],
    }
    if run_id:
        payload["run_id"] = run_id
    if extra:
        payload.update(extra)
    return payload


def execute_recorded_command(
    *,
    config: AppConfig,
    workspace: RunWorkspace,
    category: str,
    tool_name: str,
    label: str,
    plan_payload: Dict[str, Any],
    base_command: Sequence[str],
    cwd: Path,
    timeout: int,
    relative_index_path: str,
    env: Optional[Dict[str, str]] = None,
    force_nix: bool = False,
    extra_artifacts: Optional[List[str]] = None,
    paths: Optional[InvocationPaths] = None,
) -> Dict[str, Any]:
    paths = paths or create_invocation_paths(
        workspace,
        category=category,
        tool_name=tool_name,
        label=label,
    )

    plan_abs = workspace.root / paths.plan_path
    stdout_abs = workspace.root / paths.stdout_path
    stderr_abs = workspace.root / paths.stderr_path
    result_abs = workspace.root / paths.result_path
    artifacts = [paths.plan_path]
    extra_artifact_paths = list(extra_artifacts or [])

    write_json_file(plan_abs, plan_payload)

    try:
        result = run_command(
            project_root=config.project_root,
            base_command=base_command,
            cwd=cwd,
            env=env,
            timeout=timeout,
            force_nix=force_nix,
        )
        stdout_abs.write_text(result.stdout)
        stderr_abs.write_text(result.stderr)
        result_payload = {
            "command": result.command,
            "cwd": result.cwd,
            "returncode": result.returncode,
            "duration_ms": result.duration_ms,
            "stdout_path": paths.stdout_path,
            "stderr_path": paths.stderr_path,
            "extra_artifacts": extra_artifact_paths,
        }
        write_json_file(result_abs, result_payload)
        artifacts.extend([paths.stdout_path, paths.stderr_path, paths.result_path])
        artifacts.extend(extra_artifact_paths)

        status = "completed" if result.returncode == 0 else "failed"
        summary = (
            f"{tool_name} completed successfully."
            if result.returncode == 0
            else f"{tool_name} exited with code {result.returncode}."
        )
        append_index_entry(
            workspace,
            relative_index_path=relative_index_path,
            entry={
                "created_at": utc_timestamp(),
                "tool": tool_name,
                "status": status,
                "invocation_dir": paths.relative_dir,
                "command": result.command,
                "cwd": result.cwd,
                "returncode": result.returncode,
                "artifacts": artifacts,
            },
        )
        refresh_materials_manifest(config, workspace)
        return success_payload(
            tool=tool_name,
            run_id=workspace.run_id,
            status=status,
            summary=summary,
            artifacts=artifacts,
            extra={
                "command": result.command,
                "cwd": result.cwd,
                "returncode": result.returncode,
                "stdout_preview": result.stdout[:4000],
                "stderr_preview": result.stderr[:4000],
            },
        )
    except Exception as exc:
        result_payload = {
            "error": str(exc),
            "stdout_path": paths.stdout_path,
            "stderr_path": paths.stderr_path,
            "extra_artifacts": extra_artifact_paths,
        }
        stdout_abs.write_text("")
        stderr_abs.write_text(str(exc))
        write_json_file(result_abs, result_payload)
        artifacts.extend([paths.stdout_path, paths.stderr_path, paths.result_path])
        artifacts.extend(extra_artifact_paths)
        append_index_entry(
            workspace,
            relative_index_path=relative_index_path,
            entry={
                "created_at": utc_timestamp(),
                "tool": tool_name,
                "status": "error",
                "invocation_dir": paths.relative_dir,
                "artifacts": artifacts,
                "error": str(exc),
            },
        )
        refresh_materials_manifest(config, workspace)
        return error_payload(
            tool=tool_name,
            run_id=workspace.run_id,
            summary=str(exc),
            artifacts=artifacts,
        )


def spawn_logged_process(
    *,
    config: AppConfig,
    base_command: Sequence[str],
    cwd: Path,
    stdout_path: Path,
    env: Optional[Dict[str, str]] = None,
    force_nix: bool = False,
) -> tuple[subprocess.Popen[str], List[str], Any]:
    command = prefixed_command(
        project_root=config.project_root,
        base_command=base_command,
        force_nix=force_nix,
    )
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    handle = stdout_path.open("a")
    process = subprocess.Popen(
        command,
        cwd=str(cwd),
        env=merged_env,
        stdout=handle,
        stderr=subprocess.STDOUT,
        text=True,
    )
    return process, command, handle
