---
name: audit-materials
description: Aggregate prepared findings and write a neutral materials manifest for an existing run. Use when you want a stable entry point into the generated artifacts.
---

# Audit Materials

Aggregate prepared materials:

```bash
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit aggregate-materials --run-id <run_id>
```

Inspect first:

- `reports/materials_manifest.json`
- `artifacts/dependency_findings.json`

Notes:

- Repository-side findings, when present, live in `artifacts/dependency_findings.json`.
- If you save direct tool artifacts under the current `runs/<run_id>/artifacts/`, rerunning this step will surface them in `reports/materials_manifest.json` under optional tool artifacts.
