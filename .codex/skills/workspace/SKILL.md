---
name: workspace
description: Create and fully prepare a run workspace for a contract address. Use when you need a fresh runs/<run_id>/ directory with fetched source, dependency analysis, and tool workspaces already prepared.
---

# Audit Workspace

Initialize and fully prepare a run workspace:

```bash
cargo run --bin agent-audit -- init-run --chain <chain> --address <address>
```

What it does:

- Creates `runs/<run_id>/`
- Creates `input/`, `artifacts/`, `reports/`, and `logs/`
- Writes `input/request.json`
- Writes `input/run_meta.json`
- Fetches verified source into `sources/`
- Fetches discovered dependency source into `sources/dependencies/`
- Runs dependency analysis and writes `artifacts/dependency_findings.json`
- Prepares `slither_project/`, `foundry_project/`, and `echidna_project/`
- Writes `artifacts/tooling_manifest.json`
- Writes `reports/materials_manifest.json`

Inspect first:

- `reports/materials_manifest.json`
- `artifacts/source_bundle.json`
- `artifacts/dependency_findings.json`
- `artifacts/tooling_manifest.json`
