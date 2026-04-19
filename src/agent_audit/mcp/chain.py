from __future__ import annotations

import atexit
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional, TextIO

from mcp.server.fastmcp import FastMCP

from agent_audit.config import AppConfig
from agent_audit.mcp_support import (
    append_index_entry,
    create_invocation_paths,
    error_payload,
    execute_recorded_command,
    find_free_port,
    load_workspace_and_context,
    refresh_materials_manifest,
    resolve_rpc_url,
    run_lock,
    spawn_logged_process,
    success_payload,
    utc_timestamp,
    write_json_file,
)
from agent_audit.workspace import RunWorkspace


mcp = FastMCP("audit_chain")


@dataclass
class AnvilSession:
    session_id: str
    workspace: RunWorkspace
    process: Any
    command: List[str]
    log_handle: TextIO
    paths: Any
    rpc_url: str
    port: int
    fork_url: str
    fork_block_number: Optional[int]
    auto_impersonate: bool
    restart_count: int = 0


SESSIONS: Dict[str, AnvilSession] = {}


def _terminate_session(session: AnvilSession) -> int:
    if session.process.poll() is None:
        session.process.terminate()
        try:
            session.process.wait(timeout=10)
        except Exception:
            session.process.kill()
            session.process.wait(timeout=10)
    session.log_handle.flush()
    session.log_handle.close()
    return int(session.process.returncode or 0)


def _session_artifacts(session: AnvilSession) -> List[str]:
    return [
        session.paths.plan_path,
        session.paths.stdout_path,
        session.paths.stderr_path,
        session.paths.result_path,
    ]


def _write_session_result(
    session: AnvilSession,
    *,
    status: str,
    extra: Optional[Dict[str, Any]] = None,
) -> None:
    result_path = session.workspace.root / session.paths.result_path
    payload: Dict[str, Any] = {
        "session_id": session.session_id,
        "status": status,
        "rpc_url": session.rpc_url,
        "port": session.port,
        "command": session.command,
        "fork_url": session.fork_url,
        "fork_block_number": session.fork_block_number,
        "auto_impersonate": session.auto_impersonate,
        "restart_count": session.restart_count,
        "stdout_path": session.paths.stdout_path,
        "stderr_path": session.paths.stderr_path,
    }
    if extra:
        payload.update(extra)
    write_json_file(result_path, payload)


def _start_anvil_session(
    *,
    config: AppConfig,
    workspace: RunWorkspace,
    paths,
    port: int,
    fork_url: str,
    fork_block_number: Optional[int],
    auto_impersonate: bool,
    session_id: str,
) -> AnvilSession:
    stdout_abs = workspace.root / paths.stdout_path
    stderr_abs = workspace.root / paths.stderr_path
    stderr_abs.write_text("")

    base_command: List[str] = ["anvil", "--port", str(port)]
    if fork_url:
        base_command.extend(["--fork-url", fork_url])
    if fork_block_number is not None:
        base_command.extend(["--fork-block-number", str(fork_block_number)])
    if auto_impersonate:
        base_command.append("--auto-impersonate")

    process, command, handle = spawn_logged_process(
        config=config,
        base_command=base_command,
        cwd=config.project_root,
        stdout_path=stdout_abs,
    )
    time.sleep(1)
    if process.poll() is not None:
        handle.flush()
        handle.close()
        raise RuntimeError(
            f"anvil failed to start, inspect {paths.stdout_path} for startup output"
        )

    return AnvilSession(
        session_id=session_id,
        workspace=workspace,
        process=process,
        command=command,
        log_handle=handle,
        paths=paths,
        rpc_url=f"http://127.0.0.1:{port}",
        port=port,
        fork_url=fork_url,
        fork_block_number=fork_block_number,
        auto_impersonate=auto_impersonate,
    )


def _cleanup_sessions() -> None:
    for session_id in list(SESSIONS):
        session = SESSIONS.pop(session_id, None)
        if session is None:
            continue
        try:
            _terminate_session(session)
        except Exception:
            continue


atexit.register(_cleanup_sessions)


def _cast_tool(
    *,
    tool_name: str,
    run_id: str,
    label: str,
    base_command: List[str],
    plan_payload: Dict[str, Any],
    timeout: int,
) -> dict:
    config = AppConfig.load()
    try:
        workspace, _ = load_workspace_and_context(config, run_id)
        with run_lock(workspace):
            return execute_recorded_command(
                config=config,
                workspace=workspace,
                category="chain",
                tool_name=tool_name,
                label=label,
                plan_payload=plan_payload,
                base_command=base_command,
                cwd=config.project_root,
                timeout=timeout,
                relative_index_path="artifacts/chain_index.json",
            )
    except Exception as exc:
        return error_payload(tool=tool_name, run_id=run_id, summary=str(exc))


@mcp.tool()
def cast_call(
    run_id: str,
    to: str,
    signature: str,
    args: Optional[List[str]] = None,
    block: Optional[str] = None,
    rpc_url: Optional[str] = None,
    timeout: int = 120,
) -> dict:
    """Run a read-only cast call with an explicit ABI signature."""

    try:
        config = AppConfig.load()
        resolved_rpc = resolve_rpc_url(config, rpc_url)
        command = ["cast", "call", to, signature, *(args or []), "--rpc-url", resolved_rpc]
        if block:
            command.extend(["--block", str(block)])
        return _cast_tool(
            tool_name="cast_call",
            run_id=run_id,
            label=signature,
            base_command=command,
            plan_payload={
                "run_id": run_id,
                "to": to,
                "signature": signature,
                "args": args or [],
                "block": block or "",
                "rpc_url": resolved_rpc,
            },
            timeout=timeout,
        )
    except Exception as exc:
        return error_payload(tool="cast_call", run_id=run_id, summary=str(exc))


@mcp.tool()
def cast_code(
    run_id: str,
    address: str,
    block: Optional[str] = None,
    rpc_url: Optional[str] = None,
    timeout: int = 120,
) -> dict:
    """Fetch deployed bytecode for an address."""

    try:
        config = AppConfig.load()
        resolved_rpc = resolve_rpc_url(config, rpc_url)
        command = ["cast", "code", address, "--rpc-url", resolved_rpc]
        if block:
            command.extend(["--block", str(block)])
        return _cast_tool(
            tool_name="cast_code",
            run_id=run_id,
            label=address,
            base_command=command,
            plan_payload={
                "run_id": run_id,
                "address": address,
                "block": block or "",
                "rpc_url": resolved_rpc,
            },
            timeout=timeout,
        )
    except Exception as exc:
        return error_payload(tool="cast_code", run_id=run_id, summary=str(exc))


@mcp.tool()
def cast_storage(
    run_id: str,
    address: str,
    slot: str,
    block: Optional[str] = None,
    rpc_url: Optional[str] = None,
    timeout: int = 120,
) -> dict:
    """Read a storage slot from a contract address."""

    try:
        config = AppConfig.load()
        resolved_rpc = resolve_rpc_url(config, rpc_url)
        command = ["cast", "storage", address, slot, "--rpc-url", resolved_rpc]
        if block:
            command.extend(["--block", str(block)])
        return _cast_tool(
            tool_name="cast_storage",
            run_id=run_id,
            label=f"{address}_{slot}",
            base_command=command,
            plan_payload={
                "run_id": run_id,
                "address": address,
                "slot": slot,
                "block": block or "",
                "rpc_url": resolved_rpc,
            },
            timeout=timeout,
        )
    except Exception as exc:
        return error_payload(tool="cast_storage", run_id=run_id, summary=str(exc))


@mcp.tool()
def anvil_start(
    run_id: str,
    fork_url: Optional[str] = None,
    fork_block_number: Optional[int] = None,
    auto_impersonate: bool = False,
) -> dict:
    """Start an Anvil process and return a local RPC URL."""

    config = AppConfig.load()
    try:
        workspace, _ = load_workspace_and_context(config, run_id)
        with run_lock(workspace):
            resolved_fork_url = resolve_rpc_url(config, fork_url) if (fork_url or config.rpc_url) else ""
            label = "fork" if resolved_fork_url else "local"
            paths = create_invocation_paths(
                workspace,
                category="chain",
                tool_name="anvil",
                label=label,
            )
            session_id = Path(paths.relative_dir).name
            plan_path = workspace.root / paths.plan_path
            write_json_file(
                plan_path,
                {
                    "run_id": run_id,
                    "fork_url": resolved_fork_url,
                    "fork_block_number": fork_block_number,
                    "auto_impersonate": auto_impersonate,
                },
            )
            port = find_free_port()
            session = _start_anvil_session(
                config=config,
                workspace=workspace,
                paths=paths,
                port=port,
                fork_url=resolved_fork_url,
                fork_block_number=fork_block_number,
                auto_impersonate=auto_impersonate,
                session_id=session_id,
            )
            SESSIONS[session_id] = session
            _write_session_result(session, status="running", extra={"started_at": utc_timestamp()})
            artifacts = _session_artifacts(session)
            append_index_entry(
                workspace,
                relative_index_path="artifacts/chain_index.json",
                entry={
                    "created_at": utc_timestamp(),
                    "tool": "anvil_start",
                    "status": "running",
                    "session_id": session_id,
                    "rpc_url": session.rpc_url,
                    "command": session.command,
                    "artifacts": artifacts,
                },
            )
            refresh_materials_manifest(config, workspace)
            return success_payload(
                tool="anvil_start",
                run_id=run_id,
                status="completed",
                summary="anvil_start completed.",
                artifacts=artifacts,
                extra={
                    "session_id": session_id,
                    "rpc_url": session.rpc_url,
                    "command": session.command,
                },
            )
    except Exception as exc:
        return error_payload(tool="anvil_start", run_id=run_id, summary=str(exc))


@mcp.tool()
def anvil_stop(run_id: str, session_id: str) -> dict:
    """Stop a running Anvil session."""

    config = AppConfig.load()
    session = SESSIONS.get(session_id)
    if session is None:
        return error_payload(
            tool="anvil_stop",
            run_id=run_id,
            summary=f"anvil session is not active: {session_id}",
        )
    if session.workspace.run_id != run_id:
        return error_payload(
            tool="anvil_stop",
            run_id=run_id,
            summary=f"anvil session {session_id} belongs to run {session.workspace.run_id}",
        )

    try:
        with run_lock(session.workspace):
            returncode = _terminate_session(session)
            _write_session_result(
                session,
                status="stopped",
                extra={
                    "stopped_at": utc_timestamp(),
                    "returncode": returncode,
                },
            )
            append_index_entry(
                session.workspace,
                relative_index_path="artifacts/chain_index.json",
                entry={
                    "created_at": utc_timestamp(),
                    "tool": "anvil_stop",
                    "status": "completed",
                    "session_id": session_id,
                    "returncode": returncode,
                    "artifacts": _session_artifacts(session),
                },
            )
            refresh_materials_manifest(config, session.workspace)
            SESSIONS.pop(session_id, None)
            return success_payload(
                tool="anvil_stop",
                run_id=run_id,
                status="completed",
                summary="anvil_stop completed.",
                artifacts=_session_artifacts(session),
                extra={"session_id": session_id, "returncode": returncode},
            )
    except Exception as exc:
        return error_payload(tool="anvil_stop", run_id=run_id, summary=str(exc))


@mcp.tool()
def anvil_reset(
    run_id: str,
    session_id: str,
    fork_block_number: Optional[int] = None,
) -> dict:
    """Restart an Anvil session with the same port and optional new fork block."""

    config = AppConfig.load()
    session = SESSIONS.get(session_id)
    if session is None:
        return error_payload(
            tool="anvil_reset",
            run_id=run_id,
            summary=f"anvil session is not active: {session_id}",
        )
    if session.workspace.run_id != run_id:
        return error_payload(
            tool="anvil_reset",
            run_id=run_id,
            summary=f"anvil session {session_id} belongs to run {session.workspace.run_id}",
        )

    try:
        with run_lock(session.workspace):
            _terminate_session(session)
            session.restart_count += 1
            next_block = fork_block_number if fork_block_number is not None else session.fork_block_number
            stdout_abs = session.workspace.root / session.paths.stdout_path
            stdout_abs.write_text(
                stdout_abs.read_text() + f"\n# anvil reset at {utc_timestamp()}\n"
            )
            restarted = _start_anvil_session(
                config=config,
                workspace=session.workspace,
                paths=session.paths,
                port=session.port,
                fork_url=session.fork_url,
                fork_block_number=next_block,
                auto_impersonate=session.auto_impersonate,
                session_id=session.session_id,
            )
            restarted.restart_count = session.restart_count
            SESSIONS[session_id] = restarted
            _write_session_result(
                restarted,
                status="running",
                extra={
                    "reset_at": utc_timestamp(),
                    "fork_block_number": next_block,
                },
            )
            append_index_entry(
                restarted.workspace,
                relative_index_path="artifacts/chain_index.json",
                entry={
                    "created_at": utc_timestamp(),
                    "tool": "anvil_reset",
                    "status": "completed",
                    "session_id": session_id,
                    "rpc_url": restarted.rpc_url,
                    "artifacts": _session_artifacts(restarted),
                },
            )
            refresh_materials_manifest(config, restarted.workspace)
            return success_payload(
                tool="anvil_reset",
                run_id=run_id,
                status="completed",
                summary="anvil_reset completed.",
                artifacts=_session_artifacts(restarted),
                extra={
                    "session_id": session_id,
                    "rpc_url": restarted.rpc_url,
                    "fork_block_number": next_block,
                    "restart_count": restarted.restart_count,
                },
            )
    except Exception as exc:
        return error_payload(tool="anvil_reset", run_id=run_id, summary=str(exc))


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
