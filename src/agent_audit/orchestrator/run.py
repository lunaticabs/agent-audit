from __future__ import annotations

import json
import shutil
from dataclasses import dataclass
from typing import Any, Dict, List, Tuple

from agent_audit.config import AppConfig
from agent_audit.services.pipeline import AuditPipelineService
from agent_audit.workspace import RunWorkspace, generate_run_id


FAILED_STEP_STATUSES = {
    "source_api_not_configured",
    "source_fetch_failed",
    "source_bundle_missing",
    "source_not_fetched",
    "source_files_missing",
    "ir_generation_failed",
}


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


def prepare_run(
    config: AppConfig,
    *,
    address: str,
    chain: str,
) -> Tuple[RunWorkspace, Dict[str, Any]]:
    run_id = generate_run_id(address=address, chain=chain)
    staging_root = config.runs_dir / ".staging" / run_id
    workspace = RunWorkspace.create_at_root(
        project_root=config.project_root,
        root=staging_root,
        run_id=run_id,
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

    try:
        step_payloads: List[Dict[str, Any]] = []
        pipeline = AuditPipelineService(config=config, workspace=workspace)

        fetch_status = pipeline.fetch_contract_source(address=address, chain=chain)
        slither_status = "not_prepared"
        if fetch_status == "source_fetched":
            slither_status = pipeline.prepare_slither_project(
                address=address, chain=chain
            )
        fetch_payload = _step_payload(
            workspace=workspace,
            step="fetch-source",
            status=fetch_status,
            artifact_index=pipeline.write_artifact_index(),
            extra={
                "slither_project_status": slither_status,
                "slither_build_manifest_path": (
                    "slither_project/build_manifest.json"
                    if slither_status == "prepared"
                    else ""
                ),
            },
        )
        workspace.write_json("logs/fetch_source_result.json", fetch_payload)
        step_payloads.append(fetch_payload)

        ir_status = pipeline.build_ir(address=address, chain=chain)
        ir_payload = _step_payload(
            workspace=workspace,
            step="build-ir",
            status=ir_status,
            artifact_index=pipeline.write_artifact_index(),
        )
        workspace.write_json("logs/build_ir_result.json", ir_payload)
        step_payloads.append(ir_payload)

        dependency_status = pipeline.run_dependency_analysis(
            address=address, chain=chain
        )
        dependency_payload = _step_payload(
            workspace=workspace,
            step="run-dependency",
            status=dependency_status,
            artifact_index=pipeline.write_artifact_index(),
        )
        workspace.write_json("logs/run_dependency_result.json", dependency_payload)
        step_payloads.append(dependency_payload)

        prepare_slither_status = pipeline.prepare_slither_project(
            address=address, chain=chain
        )
        prepare_slither_payload = _step_payload(
            workspace=workspace,
            step="prepare-slither",
            status=prepare_slither_status,
            artifact_index=pipeline.write_artifact_index(),
            extra={
                "slither_build_manifest_path": "slither_project/build_manifest.json",
                "slither_project_root": "slither_project",
            },
        )
        workspace.write_json(
            "logs/prepare_slither_result.json", prepare_slither_payload
        )
        step_payloads.append(prepare_slither_payload)

        manifest_path = pipeline.aggregate_materials(address=address, chain=chain)
        aggregate_payload = _step_payload(
            workspace=workspace,
            step="aggregate-materials",
            status="executed",
            artifact_index=pipeline.write_artifact_index(),
            extra={
                "materials_manifest_path": manifest_path,
            },
        )
        workspace.write_json("logs/aggregate_materials_result.json", aggregate_payload)
        step_payloads.append(aggregate_payload)

        for step_payload in step_payloads:
            step_status = str(step_payload.get("status") or "")
            if step_status in FAILED_STEP_STATUSES:
                raise RuntimeError(
                    f"prepare_run failed at step {step_payload.get('step')}: {step_status}"
                )

        final_root = config.runs_dir / run_id
        final_root.parent.mkdir(parents=True, exist_ok=True)
        if final_root.exists():
            raise FileExistsError(f"run_id already exists: {run_id}")
        shutil.move(str(staging_root), str(final_root))
        staging_parent = config.runs_dir / ".staging"
        if staging_parent.exists() and not any(staging_parent.iterdir()):
            staging_parent.rmdir()

        final_workspace = RunWorkspace.load(
            project_root=config.project_root,
            runs_dir=config.runs_dir,
            run_id=run_id,
        )
        _, status_payload = get_run_status(config, final_workspace.run_id)
        payload = {
            "run_id": final_workspace.run_id,
            "run_dir": str(final_workspace.root),
            "address": address,
            "chain": chain,
            "steps": step_payloads,
            "status": status_payload,
        }
        final_workspace.write_json("logs/prepare_run_result.json", payload)
        return final_workspace, payload
    except Exception:
        shutil.rmtree(staging_root, ignore_errors=True)
        if (config.runs_dir / ".staging").exists() and not any(
            (config.runs_dir / ".staging").iterdir()
        ):
            (config.runs_dir / ".staging").rmdir()
        raise


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


def fetch_source_for_run(
    config: AppConfig, run_id: str
) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    status = pipeline.fetch_contract_source(
        address=context.address, chain=context.chain
    )
    slither_status = "not_prepared"
    if status == "source_fetched":
        slither_status = pipeline.prepare_slither_project(
            address=context.address, chain=context.chain
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


def build_ir_for_run(
    config: AppConfig, run_id: str
) -> Tuple[RunWorkspace, Dict[str, Any]]:
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


def run_dependency_for_run(
    config: AppConfig, run_id: str
) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    status = pipeline.run_dependency_analysis(
        address=context.address, chain=context.chain
    )
    payload = _step_payload(
        workspace=workspace,
        step="run-dependency",
        status=status,
        artifact_index=pipeline.write_artifact_index(),
    )
    workspace.write_json("logs/run_dependency_result.json", payload)
    return workspace, payload


def prepare_slither_for_run(
    config: AppConfig, run_id: str
) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    context = load_request_context(workspace)
    pipeline = AuditPipelineService(config=config, workspace=workspace)
    status = pipeline.prepare_slither_project(
        address=context.address, chain=context.chain
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


def get_run_status(
    config: AppConfig,
    run_id: str,
) -> Tuple[RunWorkspace, Dict[str, Any]]:
    workspace = load_workspace(config, run_id)
    request_path = workspace.root / "input" / "request.json"
    context = (
        load_request_context(workspace)
        if request_path.exists()
        else RunRequestContext("", "")
    )

    source_bundle_path = workspace.root / "artifacts" / "source_bundle.json"
    dependency_findings_path = workspace.root / "artifacts" / "dependency_findings.json"
    contracts_ir_path = workspace.root / "ir" / "contracts.json"
    slither_manifest_path = workspace.root / "slither_project" / "build_manifest.json"
    materials_manifest_path = workspace.root / "reports" / "materials_manifest.json"

    source_bundle = (
        json.loads(source_bundle_path.read_text())
        if source_bundle_path.exists()
        else {}
    )
    dependency_findings = (
        json.loads(dependency_findings_path.read_text())
        if dependency_findings_path.exists()
        else {}
    )
    contracts_ir = (
        json.loads(contracts_ir_path.read_text()) if contracts_ir_path.exists() else {}
    )
    slither_manifest = (
        json.loads(slither_manifest_path.read_text())
        if slither_manifest_path.exists()
        else {}
    )

    ir_status = str(contracts_ir.get("status") or "")
    if not ir_status and contracts_ir_path.exists():
        ir_status = "ir_generated"

    payload = {
        "run_id": workspace.run_id,
        "run_dir": str(workspace.root),
        "target": {
            "address": context.address,
            "chain": context.chain,
        },
        "statuses": {
            "source_fetch": str(source_bundle.get("status") or "not_prepared"),
            "ir": ir_status or "not_prepared",
            "dependency": str(dependency_findings.get("status") or "not_prepared"),
            "slither": str(slither_manifest.get("status") or "not_prepared"),
            "materials": "prepared"
            if materials_manifest_path.exists()
            else "not_prepared",
        },
        "paths": {
            "request": "input/request.json" if request_path.exists() else "",
            "source_bundle": "artifacts/source_bundle.json"
            if source_bundle_path.exists()
            else "",
            "contracts_ir": "ir/contracts.json" if contracts_ir_path.exists() else "",
            "dependency_findings": (
                "artifacts/dependency_findings.json"
                if dependency_findings_path.exists()
                else ""
            ),
            "slither_manifest": (
                "slither_project/build_manifest.json"
                if slither_manifest_path.exists()
                else ""
            ),
            "materials_manifest": (
                "reports/materials_manifest.json"
                if materials_manifest_path.exists()
                else ""
            ),
        },
    }
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
