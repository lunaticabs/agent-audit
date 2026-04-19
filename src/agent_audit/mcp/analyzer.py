from __future__ import annotations

import os
from pathlib import Path
from typing import List, Optional

from mcp.server.fastmcp import FastMCP

from agent_audit.config import AppConfig
from agent_audit.mcp_support import (
    create_invocation_paths,
    error_payload,
    execute_recorded_command,
    load_workspace_and_context,
    read_json_if_exists,
    run_lock,
    shell_join,
)


mcp = FastMCP("audit_analyzer")


def _resolve_project_path(config: AppConfig, value: Optional[str]) -> Path:
    if not value:
        return config.project_root
    path = (config.project_root / value).resolve()
    try:
        path.relative_to(config.project_root)
    except ValueError as exc:
        raise ValueError(f"path must stay within the project root: {value}") from exc
    return path


@mcp.tool()
def run_slither(
    run_id: str,
    target_path: Optional[str] = None,
    detectors: Optional[List[str]] = None,
    exclude_detectors: Optional[List[str]] = None,
    extra_args: Optional[List[str]] = None,
    timeout: int = 600,
) -> dict:
    """Run Slither against a chosen target inside the prepared Slither workspace."""

    config = AppConfig.load()
    try:
        workspace, _ = load_workspace_and_context(config, run_id)
        with run_lock(workspace):
            manifest_path = workspace.root / "slither_project" / "build_manifest.json"
            manifest = read_json_if_exists(manifest_path)
            if str(manifest.get("status") or "") != "prepared":
                return error_payload(
                    tool="run_slither",
                    run_id=run_id,
                    summary="slither_project/build_manifest.json is missing or not prepared",
                    artifacts=["slither_project/build_manifest.json"] if manifest_path.exists() else [],
                )

            chosen_target = target_path or str(manifest.get("preferred_target") or ".")
            remappings = [
                item for item in manifest.get("remappings", []) if isinstance(item, str) and item
            ]
            solc_args = str(manifest.get("solc_args") or "")
            solc_version = str(manifest.get("solc_version") or "")
            slither_root = workspace.root / "slither_project"

            paths = create_invocation_paths(
                workspace,
                category="analyzer",
                tool_name="run_slither",
                label=chosen_target,
            )
            raw_json_path = f"{paths.relative_dir}/slither_raw.json"
            raw_json_abs = workspace.root / raw_json_path
            raw_json_from_project = os.path.relpath(raw_json_abs, start=slither_root)

            command_parts = ["slither", chosen_target, "--solc-working-dir", ".", "--json", raw_json_from_project]
            if detectors:
                command_parts.extend(["--detect", ",".join(detectors)])
            if exclude_detectors:
                command_parts.extend(["--exclude", ",".join(exclude_detectors)])
            if remappings:
                command_parts.extend(["--solc-remaps", " ".join(remappings)])
            if solc_args:
                command_parts.extend(["--solc-args", solc_args])
            if extra_args:
                command_parts.extend(extra_args)

            script_steps = [f"cd {shell_join([str(slither_root)])}"]
            solc_select = manifest.get("solc_select")
            if isinstance(solc_select, dict) and solc_version and bool(solc_select.get("is_installed")):
                script_steps.append(shell_join(["solc-select", "use", solc_version]))
            script_steps.append(shell_join(command_parts))

            return execute_recorded_command(
                config=config,
                workspace=workspace,
                category="analyzer",
                tool_name="run_slither",
                label=chosen_target,
                plan_payload={
                    "run_id": run_id,
                    "target_path": chosen_target,
                    "detectors": detectors or [],
                    "exclude_detectors": exclude_detectors or [],
                    "extra_args": extra_args or [],
                    "raw_json_path": raw_json_path,
                    "manifest_path": "slither_project/build_manifest.json",
                },
                base_command=["zsh", "-lc", " && ".join(script_steps)],
                cwd=config.project_root,
                timeout=timeout,
                relative_index_path="artifacts/analyzer_index.json",
                force_nix=True,
                extra_artifacts=[raw_json_path],
                paths=paths,
            )
    except Exception as exc:
        return error_payload(tool="run_slither", run_id=run_id, summary=str(exc))


@mcp.tool()
def forge_build(
    run_id: str,
    project_dir: Optional[str] = None,
    extra_args: Optional[List[str]] = None,
    timeout: int = 600,
) -> dict:
    """Run forge build in the repository or a chosen project subdirectory."""

    config = AppConfig.load()
    try:
        workspace, _ = load_workspace_and_context(config, run_id)
        with run_lock(workspace):
            cwd = _resolve_project_path(config, project_dir)
            return execute_recorded_command(
                config=config,
                workspace=workspace,
                category="analyzer",
                tool_name="forge_build",
                label=str(project_dir or "project_root"),
                plan_payload={
                    "run_id": run_id,
                    "project_dir": project_dir or ".",
                    "extra_args": extra_args or [],
                },
                base_command=["forge", "build", *(extra_args or [])],
                cwd=cwd,
                timeout=timeout,
                relative_index_path="artifacts/analyzer_index.json",
            )
    except Exception as exc:
        return error_payload(tool="forge_build", run_id=run_id, summary=str(exc))


@mcp.tool()
def forge_test(
    run_id: str,
    match_contract: Optional[str] = None,
    match_test: Optional[str] = None,
    verbosity: Optional[int] = None,
    fork_rpc_url: Optional[str] = None,
    extra_args: Optional[List[str]] = None,
    timeout: int = 900,
) -> dict:
    """Run forge test with optional filters and fork URL."""

    config = AppConfig.load()
    try:
        workspace, _ = load_workspace_and_context(config, run_id)
        with run_lock(workspace):
            base_command: List[str] = ["forge", "test"]
            if match_contract:
                base_command.extend(["--match-contract", match_contract])
            if match_test:
                base_command.extend(["--match-test", match_test])
            if verbosity and verbosity > 0:
                base_command.append("-" + ("v" * verbosity))
            if fork_rpc_url:
                base_command.extend(["--fork-url", fork_rpc_url])
            if extra_args:
                base_command.extend(extra_args)

            label_parts = [part for part in [match_contract, match_test, "tests"] if part]
            return execute_recorded_command(
                config=config,
                workspace=workspace,
                category="analyzer",
                tool_name="forge_test",
                label="_".join(label_parts),
                plan_payload={
                    "run_id": run_id,
                    "match_contract": match_contract or "",
                    "match_test": match_test or "",
                    "verbosity": verbosity or 0,
                    "fork_rpc_url": fork_rpc_url or "",
                    "extra_args": extra_args or [],
                },
                base_command=base_command,
                cwd=config.project_root,
                timeout=timeout,
                relative_index_path="artifacts/analyzer_index.json",
            )
    except Exception as exc:
        return error_payload(tool="forge_test", run_id=run_id, summary=str(exc))


@mcp.tool()
def run_echidna(
    run_id: str,
    harness_path: str,
    contract_name: str,
    config_path: Optional[str] = None,
    test_limit: Optional[int] = None,
    seed: Optional[int] = None,
    extra_args: Optional[List[str]] = None,
    timeout: int = 1800,
) -> dict:
    """Run Echidna against a specific harness and contract."""

    config = AppConfig.load()
    try:
        workspace, _ = load_workspace_and_context(config, run_id)
        with run_lock(workspace):
            harness = _resolve_project_path(config, harness_path)
            base_command: List[str] = ["echidna", str(harness), "--contract", contract_name]
            if config_path:
                base_command.extend(["--config", str(_resolve_project_path(config, config_path))])
            if test_limit is not None:
                base_command.extend(["--test-limit", str(test_limit)])
            if seed is not None:
                base_command.extend(["--seed", str(seed)])
            if extra_args:
                base_command.extend(extra_args)

            return execute_recorded_command(
                config=config,
                workspace=workspace,
                category="analyzer",
                tool_name="run_echidna",
                label=contract_name,
                plan_payload={
                    "run_id": run_id,
                    "harness_path": str(harness.relative_to(config.project_root)),
                    "contract_name": contract_name,
                    "config_path": config_path or "",
                    "test_limit": test_limit,
                    "seed": seed,
                    "extra_args": extra_args or [],
                },
                base_command=base_command,
                cwd=config.project_root,
                timeout=timeout,
                relative_index_path="artifacts/analyzer_index.json",
            )
    except Exception as exc:
        return error_payload(tool="run_echidna", run_id=run_id, summary=str(exc))


def main() -> None:
    mcp.run()


if __name__ == "__main__":
    main()
