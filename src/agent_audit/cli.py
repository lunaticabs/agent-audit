from __future__ import annotations

import argparse
import json
import re
from typing import Optional

from agent_audit.config import AppConfig
from agent_audit.orchestrator.run import (
    aggregate_materials_for_run,
    build_ir_for_run,
    fetch_source_for_run,
    init_audit_run,
    run_dependency_for_run,
)


ADDRESS_RE = re.compile(r"^0x[a-fA-F0-9]{40}$")


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
    init_parser.add_argument("--address", required=True, help="Target contract address.")
    init_parser.add_argument(
        "--chain",
        default=None,
        help="Chain identifier. Defaults to AGENT_AUDIT_DEFAULT_CHAIN.",
    )

    for name, help_text in [
        ("fetch-source", "Fetch verified source into an existing run workspace."),
        ("build-ir", "Build IR for an existing run workspace."),
        ("run-dependency", "Run high-signal dependency analysis for an existing run workspace."),
        ("aggregate-materials", "Aggregate prepared findings and write neutral review materials."),
    ]:
        step_parser = subparsers.add_parser(name, help=help_text)
        step_parser.add_argument("--run-id", required=True, help="Existing run id under runs/.")
    return parser


def validate_address(address: str) -> str:
    if not ADDRESS_RE.match(address):
        raise ValueError(f"invalid EVM address: {address}")
    return address.lower()


def cmd_init_run(config: AppConfig, address: str, chain: Optional[str]) -> int:
    address = validate_address(address)
    chain = chain or config.default_chain
    workspace = init_audit_run(
        config,
        address=address,
        chain=chain,
    )
    payload = {
        "run_id": workspace.run_id,
        "run_dir": str(workspace.root),
        "address": address,
        "chain": chain,
    }
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    return 0


def _print_step_result(payload: object) -> int:
    print(json.dumps(payload, indent=2, ensure_ascii=False))
    return 0


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    config = AppConfig.load()

    if args.command == "init-run":
        return cmd_init_run(
            config=config,
            address=args.address,
            chain=args.chain,
        )
    if args.command == "fetch-source":
        _, payload = fetch_source_for_run(config, args.run_id)
        return _print_step_result(payload)
    if args.command == "build-ir":
        _, payload = build_ir_for_run(config, args.run_id)
        return _print_step_result(payload)
    if args.command == "run-dependency":
        _, payload = run_dependency_for_run(config, args.run_id)
        return _print_step_result(payload)
    if args.command == "aggregate-materials":
        _, payload = aggregate_materials_for_run(config, args.run_id)
        return _print_step_result(payload)

    parser.error(f"unknown command: {args.command}")
    return 2
