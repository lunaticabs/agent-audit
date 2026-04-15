---
name: smart-contract-audit
description: Coordinate the repository's Codex-first smart contract audit workflow. Use this skill when you need to decide which local audit-material preparation step to run next, inspect run artifacts under runs/, or choose whether deeper manual or tool-driven validation is warranted.
---

## Defaults

- Trust the repository-local `.env` for `AGENT_AUDIT_*` configuration.
- Prefix every `uv` command with `UV_CACHE_DIR=/tmp/uv-cache`.
- Execute repository CLI steps sequentially for a single `run_id`.
- Treat every generated file as review material, not as a final audit conclusion.

## Tool Skills

Use these repo skills in `.agents/skills/` as needed:

- `$audit-workspace`
- `$audit-source-fetch`
- `$audit-ir-builder`
- `$audit-dependency-scan`
- `$audit-slither`
- `$audit-materials`
- `$audit-echidna`
- `$foundry-forge`
- `$foundry-cast`
- `$foundry-anvil`
- `$foundry-chisel`

## Recommended Workflow

The repository CLI prepares deterministic materials only.

Suggested order:

1. `init-run`
2. `fetch-source`
3. `build-ir`
4. `run-dependency`
5. `aggregate-materials`

After that:

- inspect `reports/materials_manifest.json`
- inspect raw evidence files
- decide whether use tools like: Slither, Echidna, Forge, Cast, or Anvil
- if you run direct tools, save their artifacts under the same `runs/<run_id>/artifacts/` tree
- rerun `aggregate-materials` if you want the manifest to list those optional artifacts

Repository-side findings, when present, live in `artifacts/dependency_findings.json`.

Finally, if you think you have figured out the all real vulnerabilities, or the contract is safe, give a report in JSON, and save it in `runs/<run_id>/reprots/final_reprot.json`.

## Primary Files

- `input/request.json`
- `artifacts/source_bundle.json`
- `ir/contracts.json`
- `ir/functions.json`
- `ir/privilege_matrix.json`
- `artifacts/dependency_findings.json`
- `reports/materials_manifest.json`
