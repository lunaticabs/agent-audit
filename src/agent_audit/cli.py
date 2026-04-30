from __future__ import annotations

import argparse
import json
import re
from typing import Any, Dict, Optional

from agent_audit.config import AppConfig
from agent_audit.services.pipeline import AuditPipelineService
from agent_audit.workspace import (
    RunWorkspace,
    load_request_context,
    run_lock,
)


ADDRESS_RE = re.compile(r"^0x[a-fA-F0-9]{40}$")

EXIT_OK = 0
EXIT_RETRYABLE = 10
EXIT_FATAL = 20
EXIT_PRECONDITION = 30

RETRYABLE_STATUSES = {
    "source_fetch_failed",
}

PRECONDITION_STATUSES = {
    "source_not_fetched",
    "source_files_missing",
}

PREREQUISITE_BY_COMMAND = {
    "run-dependency": "fetch-source",
    "prepare-slither": "fetch-source",
}


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="agent-audit",
        description="Run the local smart contract audit pipeline scaffold.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    init_parser = subparsers.add_parser(
        "init-run",
        help="Create a run workspace without executing any audit steps.",
    )
    init_parser.add_argument(
        "--address", required=True, help="Target contract address."
    )
    init_parser.add_argument(
        "--chain",
        default=None,
        help="Chain identifier. Defaults to AGENT_AUDIT_DEFAULT_CHAIN.",
    )

    for name, help_text in [
        ("fetch-source", "Fetch verified source into an existing run workspace."),
        (
            "run-dependency",
            "Run high-signal dependency analysis for an existing run workspace.",
        ),
        (
            "prepare-slither",
            "Prepare an import-compatible Slither project workspace for an existing run.",
        ),
        (
            "aggregate-materials",
            "Aggregate prepared findings and write neutral review materials.",
        ),
        ("sync-run", "Sync an existing successful run into MongoDB."),
    ]:
        step_parser = subparsers.add_parser(name, help=help_text)
        step_parser.add_argument(
            "--run-id", required=True, help="Existing run id under runs/."
        )
    return parser


def validate_address(address: str) -> str:
    if not ADDRESS_RE.match(address):
        raise ValueError(f"invalid EVM address: {address}")
    return address.lower()


def init_audit_run(config: AppConfig, address: str, chain: str) -> RunWorkspace:
    workspace = RunWorkspace.create(
        project_root=config.project_root,
        runs_dir=config.runs_dir,
        address=address,
        chain=chain,
    )
    workspace.write_json(
        "input/request.json",
        {
            "address": address,
            "chain": chain,
        },
    )
    return workspace


def load_workspace(config: AppConfig, run_id: str) -> RunWorkspace:
    return RunWorkspace.load(
        project_root=config.project_root,
        runs_dir=config.runs_dir,
        run_id=run_id,
    )


def cmd_init_run(config: AppConfig, address: str, chain: Optional[str]) -> int:
    address = validate_address(address)
    chain = chain or config.default_chain
    workspace = init_audit_run(
        config,
        address=address,
        chain=chain,
    )
    payload: Dict[str, Any] = {
        "run_id": workspace.run_id,
        "run_dir": str(workspace.root),
        "address": address,
        "chain": chain,
    }
    envelope = {
        "ok": True,
        "status": "completed",
        "retryable": False,
        "run_id": workspace.run_id,
        "run_persisted": True,
        "payload": payload,
        "next_action": {
            "type": "continue",
            "command": f"UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit fetch-source --run-id {workspace.run_id}",
        },
    }
    print(json.dumps(envelope, indent=2, ensure_ascii=False))
    return EXIT_OK


def _step_envelope(
    command: str, run_id: str, payload: Dict[str, Any]
) -> tuple[Dict[str, Any], int]:
    status = str(payload.get("status") or "")
    if status in RETRYABLE_STATUSES:
        return (
            {
                "ok": False,
                "status": "retryable_error",
                "retryable": True,
                "run_id": run_id,
                "run_persisted": True,
                "payload": payload,
                "next_action": {
                    "type": "retry_same_command",
                    "command": f"UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit {command} --run-id {run_id}",
                    "retry_after_sec": 5,
                    "max_retries": 3,
                },
            },
            EXIT_RETRYABLE,
        )
    if status in PRECONDITION_STATUSES:
        prerequisite = PREREQUISITE_BY_COMMAND.get(command, "init-run")
        next_command = (
            f"UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit {prerequisite} --run-id {run_id}"
            if prerequisite != "init-run"
            else "UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit init-run --chain <chain> --address <address>"
        )
        return (
            {
                "ok": False,
                "status": "precondition_missing",
                "retryable": False,
                "run_id": run_id,
                "run_persisted": True,
                "payload": payload,
                "next_action": {
                    "type": "run_prerequisite",
                    "command": next_command,
                },
            },
            EXIT_PRECONDITION,
        )
    if status == "source_api_not_configured":
        return (
            {
                "ok": False,
                "status": "fatal_error",
                "retryable": False,
                "run_id": run_id,
                "run_persisted": True,
                "payload": payload,
                "error": {
                    "code": "SOURCE_API_NOT_CONFIGURED",
                    "message": "Configure AGENT_AUDIT_SOURCE_API_BASE before fetch-source.",
                },
                "next_action": {
                    "type": "stop",
                    "command": "set AGENT_AUDIT_SOURCE_API_BASE in .env",
                },
            },
            EXIT_FATAL,
        )
    return (
        {
            "ok": True,
            "status": "completed",
            "retryable": False,
            "run_id": run_id,
            "run_persisted": True,
            "payload": payload,
            "next_action": {
                "type": "continue",
            },
        },
        EXIT_OK,
    )


def _print_json(payload: Dict[str, Any]) -> None:
    print(json.dumps(payload, indent=2, ensure_ascii=False))


def _step_payload(
    *,
    workspace: RunWorkspace,
    step: str,
    status: str,
    artifact_index: str,
    extra: Dict[str, Any] | None = None,
) -> Dict[str, Any]:
    payload: Dict[str, Any] = {
        "run_id": workspace.run_id,
        "run_dir": str(workspace.root),
        "step": step,
        "status": status,
        "artifact_index": artifact_index,
    }
    if extra:
        payload.update(extra)
    return payload


def _load_pipeline(
    config: AppConfig, run_id: str
) -> tuple[RunWorkspace, Dict[str, str], AuditPipelineService]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    return (
        workspace,
        {"address": context.address, "chain": context.chain},
        pipeline,
    )


def cmd_fetch_source(config: AppConfig, run_id: str) -> tuple[RunWorkspace, Dict[str, Any]]:
    workspace, context, pipeline = _load_pipeline(config, run_id)
    status = pipeline.fetch_contract_source(
        address=context["address"],
        chain=context["chain"],
    )
    slither_status = "not_prepared"
    if status == "source_fetched":
        slither_status = pipeline.prepare_slither_project(
            address=context["address"],
            chain=context["chain"],
        )
    payload = _step_payload(
        workspace=workspace,
        step="fetch-source",
        status=status,
        artifact_index=pipeline.write_artifact_index(),
        extra={
            "slither_project_status": slither_status,
            "slither_build_manifest_path": "slither_project/build_manifest.json"
            if slither_status == "prepared"
            else "",
        },
    )
    workspace.write_json("logs/fetch_source_result.json", payload)
    return workspace, payload


def cmd_run_dependency(
    config: AppConfig, run_id: str
) -> tuple[RunWorkspace, Dict[str, Any]]:
    workspace, context, pipeline = _load_pipeline(config, run_id)
    status = pipeline.run_dependency_analysis(
        address=context["address"],
        chain=context["chain"],
    )
    payload = _step_payload(
        workspace=workspace,
        step="run-dependency",
        status=status,
        artifact_index=pipeline.write_artifact_index(),
    )
    workspace.write_json("logs/run_dependency_result.json", payload)
    return workspace, payload


def cmd_prepare_slither(
    config: AppConfig, run_id: str
) -> tuple[RunWorkspace, Dict[str, Any]]:
    workspace, context, pipeline = _load_pipeline(config, run_id)
    status = pipeline.prepare_slither_project(
        address=context["address"],
        chain=context["chain"],
    )
    payload = _step_payload(
        workspace=workspace,
        step="prepare-slither",
        status=status,
        artifact_index=pipeline.write_artifact_index(),
        extra={
            "slither_build_manifest_path": "slither_project/build_manifest.json",
            "slither_project_root": "slither_project",
        },
    )
    workspace.write_json("logs/prepare_slither_result.json", payload)
    return workspace, payload


def cmd_aggregate_materials(
    config: AppConfig, run_id: str
) -> tuple[RunWorkspace, Dict[str, Any]]:
    workspace, context, pipeline = _load_pipeline(config, run_id)
    manifest_path = pipeline.aggregate_materials(
        address=context["address"],
        chain=context["chain"],
    )
    payload = _step_payload(
        workspace=workspace,
        step="aggregate-materials",
        status="executed",
        artifact_index=pipeline.write_artifact_index(),
        extra={
            "materials_manifest_path": manifest_path,
        },
    )
    workspace.write_json("logs/aggregate_materials_result.json", payload)
    return workspace, payload


def _same_command(args: argparse.Namespace) -> str:
    command = str(getattr(args, "command", "") or "")
    run_id = str(getattr(args, "run_id", "") or "")
    if command and run_id:
        return (
            f"UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit {command} --run-id {run_id}"
        )
    return "UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit <same-command>"


def _error_envelope(
    args: argparse.Namespace, exc: Exception
) -> tuple[Dict[str, Any], int]:
    run_id = getattr(args, "run_id", "")
    summary = str(exc)
    if isinstance(exc, FileNotFoundError):
        return (
            {
                "ok": False,
                "status": "precondition_missing",
                "retryable": False,
                "run_id": run_id,
                "run_persisted": False,
                "error": {
                    "code": "RUN_NOT_FOUND",
                    "message": summary,
                },
                "next_action": {
                    "type": "run_prerequisite",
                    "command": "UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit init-run --chain <chain> --address <address>",
                },
            },
            EXIT_PRECONDITION,
        )
    if isinstance(exc, ValueError):
        return (
            {
                "ok": False,
                "status": "fatal_error",
                "retryable": False,
                "run_id": run_id,
                "run_persisted": bool(run_id),
                "error": {
                    "code": "INVALID_ARGUMENT",
                    "message": summary,
                },
                "next_action": {
                    "type": "stop",
                },
            },
            EXIT_FATAL,
        )
    return (
        {
            "ok": False,
            "status": "retryable_error",
            "retryable": True,
            "run_id": run_id,
            "run_persisted": bool(run_id),
            "error": {
                "code": "UNHANDLED_EXCEPTION",
                "message": summary,
            },
            "next_action": {
                "type": "retry_same_command",
                "command": _same_command(args),
                "retry_after_sec": 5,
                "max_retries": 2,
            },
        },
        EXIT_RETRYABLE,
    )


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    config = AppConfig.load()
    try:
        if args.command == "init-run":
            return cmd_init_run(
                config=config,
                address=args.address,
                chain=args.chain,
            )
        if args.command == "fetch-source":
            workspace = load_workspace(config, args.run_id)
            with run_lock(workspace):
                _, payload = cmd_fetch_source(config, args.run_id)
            envelope, code = _step_envelope("fetch-source", args.run_id, payload)
            _print_json(envelope)
            return code
        if args.command == "run-dependency":
            workspace = load_workspace(config, args.run_id)
            with run_lock(workspace):
                _, payload = cmd_run_dependency(config, args.run_id)
            envelope, code = _step_envelope("run-dependency", args.run_id, payload)
            _print_json(envelope)
            return code
        if args.command == "prepare-slither":
            workspace = load_workspace(config, args.run_id)
            with run_lock(workspace):
                _, payload = cmd_prepare_slither(config, args.run_id)
            envelope, code = _step_envelope("prepare-slither", args.run_id, payload)
            _print_json(envelope)
            return code
        if args.command == "aggregate-materials":
            workspace = load_workspace(config, args.run_id)
            with run_lock(workspace):
                workspace, payload = cmd_aggregate_materials(config, args.run_id)
            envelope, code = _step_envelope("aggregate-materials", args.run_id, payload)
            _print_json(envelope)
            return code
        if args.command == "sync-run":
            from agent_audit.mongo_store import sync_run_to_mongo

            workspace = load_workspace(config, args.run_id)
            sync = sync_run_to_mongo(config, workspace)
            _print_json(
                {
                    "ok": True,
                    "status": "completed",
                    "retryable": False,
                    "run_id": sync.run_id,
                    "run_persisted": True,
                    "mongo_sync": {
                        "status": "completed",
                        "file_count": sync.file_count,
                        "total_size_bytes": sync.total_size_bytes,
                        "upserted_file_records": sync.upserted_file_records,
                    },
                    "next_action": {
                        "type": "continue",
                    },
                }
            )
            return EXIT_OK

        parser.error(f"unknown command: {args.command}")
        return 2
    except Exception as exc:
        envelope, code = _error_envelope(args, exc)
        _print_json(envelope)
        return code
