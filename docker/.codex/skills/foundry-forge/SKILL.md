---
name: foundry-forge
description: Use Foundry's forge tool in the CLI-prepared run workspace. Use when you need to compile contracts, run targeted tests, run fork tests, replay failures, or execute Solidity scripts for local simulation.
---

# Foundry Forge

Run Forge from `foundry_project/build_manifest.json`'s `preferred_working_dir`.

Use this workflow:

1. Ensure `foundry_project/build_manifest.json` exists. Normally `$workspace` already prepares it. If needed, rerun `agent-audit prepare-tooling --run-id <run_id>`.
2. `cd runs/<run_id>/foundry_project`
3. Use `preferred_target`, `solc_version`, and `remappings` from the manifest.

RPC source:

- When a script or fork test needs a live RPC, default to the repository-local `.env` value in `AGENT_AUDIT_RPC_URL`.
- Direct shell commands need the current shell to load `.env` first so `"$AGENT_AUDIT_RPC_URL"` expands correctly.
- Only override it when the task intentionally targets another chain or a local Anvil node.

For audit work, the most relevant workflows are:

- build:

```bash
cd runs/<run_id>/foundry_project && forge build
```

- run all tests:

```bash
cd runs/<run_id>/foundry_project && forge test
```

- run targeted tests:

```bash
cd runs/<run_id>/foundry_project && forge test --match-contract <ContractTest> --match-test <test_name>
```

- get traces:

```bash
cd runs/<run_id>/foundry_project && forge test -vvvv
```

- run a Solidity script in simulation or fork context:

```bash
source .env && cd runs/<run_id>/foundry_project && forge script script/<Name>.s.sol --fork-url "$AGENT_AUDIT_RPC_URL"
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
- Do not assume the prepared project already has tests or scripts; inspect `foundry_project/` first.
- Save the exact command, target, and intent in `artifacts/forge_plan.json`.
- Save raw test or script output in `artifacts/forge_output.txt`.
- If you summarize a reproduced issue or invariant break, save it in `artifacts/forge_findings.json`.
- Save any ad-hoc test, script, or harness source under `sources/forge/`.
- Rerun `agent-audit aggregate-materials --run-id <run_id>` if you want the manifest to list these optional artifacts.

Official docs:

- Overview: https://getfoundry.sh/forge
- Tests overview: https://getfoundry.sh/forge/tests/overview/
- Solidity scripting: https://getfoundry.sh/guides/scripting-with-solidity/
