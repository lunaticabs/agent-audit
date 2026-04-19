from __future__ import annotations

from typing import Optional

from mcp.server.fastmcp import FastMCP

from agent_audit.cli import validate_address
from agent_audit.config import AppConfig
from agent_audit.mcp_support import error_payload, run_lock, success_payload
from agent_audit.orchestrator.run import (
    aggregate_materials_for_run,
    build_ir_for_run,
    fetch_source_for_run,
    get_run_status as get_run_status_payload,
    init_audit_run,
    load_workspace,
    prepare_run as prepare_run_workflow,
    prepare_slither_for_run,
    run_dependency_for_run,
)


mcp = FastMCP("audit_pipeline")


def _single_step(
    *,
    tool_name: str,
    run_id: str,
    runner,
) -> dict:
    config = AppConfig.load()
    try:
        workspace = load_workspace(config, run_id)
        with run_lock(workspace):
            _, payload = runner(config, run_id)
        return success_payload(
            tool=tool_name,
            run_id=workspace.run_id,
            status="completed",
            summary=f"{tool_name} completed.",
            artifacts=[str(payload.get("artifact_index") or "")] if payload.get("artifact_index") else [],
            extra={"payload": payload},
        )
    except Exception as exc:
        return error_payload(tool=tool_name, run_id=run_id, summary=str(exc))


@mcp.tool()
def prepare_run(address: str, chain: Optional[str] = None) -> dict:
    """Create a new run and prepare deterministic review materials for it."""

    config = AppConfig.load()
    try:
        normalized_address = validate_address(address)
        normalized_chain = chain or config.default_chain
        _, payload = prepare_run_workflow(
            config,
            address=normalized_address,
            chain=normalized_chain,
        )
        return success_payload(
            tool="prepare_run",
            run_id=str(payload["run_id"]),
            status="completed",
            summary="prepare_run completed.",
            artifacts=[
                "logs/prepare_run_result.json",
                str(payload["status"]["paths"].get("source_bundle") or ""),
                str(payload["status"]["paths"].get("contracts_ir") or ""),
                str(payload["status"]["paths"].get("dependency_findings") or ""),
                str(payload["status"]["paths"].get("slither_manifest") or ""),
                str(payload["status"]["paths"].get("materials_manifest") or ""),
            ],
            extra={"payload": payload},
        )
    except Exception as exc:
        return error_payload(tool="prepare_run", summary=str(exc))


@mcp.tool()
def init_run(address: str, chain: Optional[str] = None) -> dict:
    """Create a run workspace without executing analysis steps."""

    config = AppConfig.load()
    try:
        normalized_address = validate_address(address)
        normalized_chain = chain or config.default_chain
        workspace = init_audit_run(
            config,
            address=normalized_address,
            chain=normalized_chain,
        )
        result_payload = {
            "run_id": workspace.run_id,
            "run_dir": str(workspace.root),
            "address": normalized_address,
            "chain": normalized_chain,
        }
        return success_payload(
            tool="init_run",
            run_id=workspace.run_id,
            status="completed",
            summary="init_run completed.",
            artifacts=["input/request.json"],
            extra={"payload": result_payload},
        )
    except Exception as exc:
        return error_payload(tool="init_run", summary=str(exc))


@mcp.tool()
def fetch_source(run_id: str) -> dict:
    """Fetch verified source materials into an existing run."""

    return _single_step(tool_name="fetch_source", run_id=run_id, runner=fetch_source_for_run)


@mcp.tool()
def build_ir(run_id: str) -> dict:
    """Build lightweight IR artifacts for an existing run."""

    return _single_step(tool_name="build_ir", run_id=run_id, runner=build_ir_for_run)


@mcp.tool()
def run_dependency(run_id: str) -> dict:
    """Run dependency discovery and repository-side dependency checks."""

    return _single_step(tool_name="run_dependency", run_id=run_id, runner=run_dependency_for_run)


@mcp.tool()
def prepare_slither(run_id: str) -> dict:
    """Prepare the import-compatible Slither workspace for a run."""

    return _single_step(tool_name="prepare_slither", run_id=run_id, runner=prepare_slither_for_run)


@mcp.tool()
def aggregate_materials(run_id: str) -> dict:
    """Refresh the neutral materials manifest for a run."""

    return _single_step(
        tool_name="aggregate_materials",
        run_id=run_id,
        runner=aggregate_materials_for_run,
    )


@mcp.tool()
def get_run_status(run_id: str) -> dict:
    """Return a concise status snapshot for an existing run."""

    config = AppConfig.load()
    try:
        _, payload = get_run_status_payload(config, run_id)
        return success_payload(
            tool="get_run_status",
            run_id=run_id,
            status="completed",
            summary="get_run_status completed.",
            artifacts=[path for path in payload["paths"].values() if path],
            extra={"payload": payload},
        )
    except Exception as exc:
        return error_payload(tool="get_run_status", run_id=run_id, summary=str(exc))


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
