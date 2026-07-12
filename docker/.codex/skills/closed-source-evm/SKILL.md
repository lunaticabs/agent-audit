---
name: closed-source-evm
description: Audit unverified or closed-source EVM contracts, closed proxy implementations, and bytecode-only targets prepared by agent-audit.
---

# Closed-Source EVM Review

Use this when the target contract, a proxy implementation, or a dependency is unverified, closed source, bytecode-only, or listed in `artifacts/bytecode_targets.json`.

Start by reading these run artifacts:

- `runs/<run_id>/artifacts/source_bundle.json`
- `runs/<run_id>/artifacts/bytecode_targets.json`
- `runs/<run_id>/artifacts/heimdall/build_manifest.json`
- `runs/<run_id>/artifacts/proxy_checks.json`
- `runs/<run_id>/reports/materials_manifest.json`

For each high-risk target in `bytecode_targets.json`, inspect:

- `role`
- `origin`
- `origin_evidence`
- `source_availability`
- `bytecode_status`
- `bytecode_artifact`

If `bytecode_status` is `rpc_not_configured`, do not claim bytecode was reviewed. Configure RPC or record the target as blocked by missing RPC.

## Heimdall

Do not create Heimdall directories or redirect output by hand. The CLI prepares
managed Heimdall command workspaces through:

```bash
agent-audit prepare-tooling --run-id <run_id>
```

Then inspect `runs/<run_id>/artifacts/heimdall/build_manifest.json`. For each
high-risk target, run the prepared script for the desired action, for example:

```bash
bash runs/<run_id>/artifacts/heimdall/<chain>/<address>/decompile/run.sh
bash runs/<run_id>/artifacts/heimdall/<chain>/<address>/disassemble/run.sh
bash runs/<run_id>/artifacts/heimdall/<chain>/<address>/cfg/run.sh
```

The prepared scripts save `command.txt`, `stdout.txt`, `stderr.txt`,
`exit_code.txt`, optional `failure.txt`, and the Heimdall output directory under
the same action directory. Rerun `agent-audit aggregate-materials --run-id
<run_id>` after running Heimdall so the material manifest includes the new
outputs.

## Review Focus

Prioritize:

- upgradeability and proxy slot behavior
- `delegatecall`, `callcode`, and fallback dispatch
- access-control gates and privileged selectors
- external calls and value transfers
- storage writes and authorization-sensitive state changes

For economic-security coverage, use the `$close-source-fuzzing` workflow early. Closed-source review should not conclude from selector matching or decompiler inspection alone when fork-backed Foundry or Echidna fuzzing can check concrete balance, token, debt, collateral, approval, and accounting properties.

Treat Heimdall decompiler output as heuristic evidence. Do not treat pseudocode or reconstructed Solidity/Yul as verified source. A confirmed finding still needs a concrete run artifact reference, such as `bytecode_targets.json`, runtime bytecode, `proxy_checks.json`, Heimdall output, or a reproducible command/test artifact.
