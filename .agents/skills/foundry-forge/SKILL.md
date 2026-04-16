---
name: foundry-forge
description: Use Foundry's forge tool for Solidity build, test, traces, and scripts in this repository. Use when you need to compile contracts, run targeted tests, run fork tests, replay failures, or execute Solidity scripts for local simulation.
---

# Foundry Forge

Use `forge` through the repository devShell:

```bash
nix develop .#default -c forge <subcommand> ...
```

RPC source:

- When a script or fork test needs a live RPC, default to the repository-local `.env` value in `AGENT_AUDIT_RPC_URL`.
- Direct shell commands need the current shell to load `.env` first so `"$AGENT_AUDIT_RPC_URL"` expands correctly.
- Only override it when the task intentionally targets another chain or a local Anvil node.

For audit work, the most relevant workflows are:

- build:

```bash
nix develop .#default -c forge build
```

- run all tests:

```bash
nix develop .#default -c forge test
```

- run targeted tests:

```bash
nix develop .#default -c forge test --match-contract <ContractTest> --match-test <test_name>
```

- get traces:

```bash
nix develop .#default -c forge test -vvvv
```

- run a Solidity script in simulation or fork context:

```bash
nix develop .#default -c bash -lc 'source .env && forge script script/<Name>.s.sol --fork-url "$AGENT_AUDIT_RPC_URL"'
```

Default artifact convention for a current run:

```text
runs/<run_id>/artifacts/forge_plan.json
runs/<run_id>/artifacts/forge_output.txt
runs/<run_id>/artifacts/forge_findings.json
runs/<run_id>/sources/forge/
```

Audit guidance:

- Prefer `forge build` and `forge test` before writing custom harnesses.
- Prefer targeted filters over whole-project test runs when validating a specific suspicion.
- Use `-vvvv` when the trace matters.
- `forge script` is useful for local simulation and fork reproduction.
- Do not add `--broadcast` unless the user explicitly asks for a live transaction.
- Do not assume a project already has tests or scripts; inspect the repo first.
- Save the exact command, target, and intent in `artifacts/forge_plan.json`.
- Save raw test or script output in `artifacts/forge_output.txt`.
- If you summarize a reproduced issue or invariant break, save it in `artifacts/forge_findings.json`.
- Save any ad-hoc test, script, or harness source under `sources/forge/`.
- Rerun `agent-audit aggregate-materials --run-id <run_id>` if you want the manifest to list these optional artifacts.

Official docs:

- Overview: https://getfoundry.sh/forge
- Tests overview: https://getfoundry.sh/forge/tests/overview/
- Solidity scripting: https://getfoundry.sh/guides/scripting-with-solidity/
