---
name: aggregate-materials
description: Aggregate prepared findings and write a neutral materials manifest for an existing run inside the containerized audit runtime.
---

# Audit Materials

Aggregate prepared materials:

```bash
agent-audit aggregate-materials --run-id <run_id>
```

Inspect first:

- `runs/<run_id>/reports/materials_manifest.json`
- `runs/<run_id>/artifacts/dependency_findings.json`

Notes:

- Repository-side findings, when present, live in `runs/<run_id>/artifacts/dependency_findings.json`.
- If you save direct tool artifacts under the current `runs/<run_id>/artifacts/`, rerunning this step will surface them in `runs/<run_id>/reports/materials_manifest.json` under optional tool artifacts.
