---
name: workspace
description: Create and fully prepare a run workspace for a contract address inside the containerized audit runtime.
---

# Audit Workspace

Initialize and fully prepare a run workspace:

```bash
agent-audit init-run --chain <chain> --address <address>
```

What it does:

- Creates `runs/<run_id>/`
- Creates `runs/<run_id>/input/`, `runs/<run_id>/artifacts/`, `runs/<run_id>/reports/`, and `runs/<run_id>/logs/`
- Writes `runs/<run_id>/input/request.json`
- Writes `runs/<run_id>/input/run_meta.json`
- Fetches verified source into `runs/<run_id>/sources/`
- Fetches discovered dependency source into `runs/<run_id>/sources/dependencies/`
- Runs dependency analysis and writes `runs/<run_id>/artifacts/dependency_findings.json`
- Prepares `runs/<run_id>/slither_project/`, `runs/<run_id>/foundry_project/`, and `runs/<run_id>/echidna_project/`
- Writes `runs/<run_id>/artifacts/tooling_manifest.json`
- Writes `runs/<run_id>/reports/materials_manifest.json`

Inspect first:

- `runs/<run_id>/reports/materials_manifest.json`
- `runs/<run_id>/artifacts/source_bundle.json`
- `runs/<run_id>/artifacts/dependency_findings.json`
- `runs/<run_id>/artifacts/tooling_manifest.json`
