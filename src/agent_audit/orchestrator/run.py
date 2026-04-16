from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Dict, Tuple

from agent_audit.config import AppConfig
from agent_audit.services.pipeline import AuditPipelineService
from agent_audit.workspace import RunWorkspace


@dataclass(frozen=True)
class RunRequestContext:
    address: str
    chain: str


def init_audit_run(
    config: AppConfig,
    *,
    address: str,
    chain: str,
) -> RunWorkspace:
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


def load_request_context(workspace: RunWorkspace) -> RunRequestContext:
    request_path = workspace.root / "input" / "request.json"
    if not request_path.exists():
        raise FileNotFoundError(
            f"missing request context for run_id {workspace.run_id}: {request_path}"
        )
    payload = json.loads(request_path.read_text())
    return RunRequestContext(
        address=str(payload.get("address") or ""),
        chain=str(payload.get("chain") or ""),
    )


def fetch_source_for_run(config: AppConfig, run_id: str) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    status = pipeline.fetch_contract_source(address=context.address, chain=context.chain)
    slither_status = "not_prepared"
    if status == "source_fetched":
        slither_status = pipeline.prepare_slither_project(address=context.address, chain=context.chain)
    payload = _step_payload(
        workspace=workspace,
        step="fetch-source",
        status=status,
        artifact_index=pipeline.write_artifact_index(),
        extra={
            "slither_project_status": slither_status,
            "slither_build_manifest_path": "slither_project/build_manifest.json" if slither_status == "prepared" else "",
        },
    )
    workspace.write_json("logs/fetch_source_result.json", payload)
    return workspace, payload


def build_ir_for_run(config: AppConfig, run_id: str) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    status = pipeline.build_ir(address=context.address, chain=context.chain)
    payload = _step_payload(
        workspace=workspace,
        step="build-ir",
        status=status,
        artifact_index=pipeline.write_artifact_index(),
    )
    workspace.write_json("logs/build_ir_result.json", payload)
    return workspace, payload


def run_dependency_for_run(config: AppConfig, run_id: str) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    status = pipeline.run_dependency_analysis(address=context.address, chain=context.chain)
    payload = _step_payload(
        workspace=workspace,
        step="run-dependency",
        status=status,
        artifact_index=pipeline.write_artifact_index(),
    )
    workspace.write_json("logs/run_dependency_result.json", payload)
    return workspace, payload


def prepare_slither_for_run(config: AppConfig, run_id: str) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    status = pipeline.prepare_slither_project(address=context.address, chain=context.chain)
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


def aggregate_materials_for_run(
    config: AppConfig,
    run_id: str,
) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    manifest_path = pipeline.aggregate_materials(
        address=context.address,
        chain=context.chain,
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
