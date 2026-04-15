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

Use these repo skills as needed:

- `$audit-workspace`
- `$audit-source-fetch`
- `$audit-chain-checks`
- `$audit-ir-builder`
- `$audit-dependency-scan`
- `$audit-slither`
- `$audit-materials`
- `$audit-echidna`
- `$foundry-forge`
- `$foundry-cast`
- `$foundry-anvil`
- `$foundry-chisel`

## Recommended Review Order

1. Create or load a run workspace.
2. Fetch verified source.
3. Build IR.
4. Run dependency analysis.
5. Aggregate neutral materials.
6. Read `reports/materials_manifest.json` and then inspect raw evidence files directly.
7. If live-chain context is useful, decide whether to invoke direct chain checks with Cast.
8. If static analysis is worth the noise for this target, decide whether to invoke Slither directly.
9. If a concrete hypothesis needs dynamic validation, decide whether to invoke Echidna, Forge, Anvil, or Cast directly.

## Primary Files

- `input/request.json`
- `artifacts/source_bundle.json`
- `ir/contracts.json`
- `ir/functions.json`
- `ir/privilege_matrix.json`
- `artifacts/dependency_findings.json`
- `reports/materials_manifest.json`

## Notes

- Repository-side findings, when present, live in `artifacts/dependency_findings.json`.
- When you invoke direct tools such as chain checks, Slither, Echidna, Forge, Cast, or Anvil, prefer saving evidence under the current `runs/<run_id>/artifacts/` directory.
- After saving direct tool artifacts, rerun `$audit-materials` if you want `reports/materials_manifest.json` to pick them up.
- Workflow notes live in `references/workflow.md`.
