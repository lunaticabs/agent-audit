---
name: audit-dependency-scan
description: Run repository dependency discovery and role-specific dependency analysis for an existing run. Use when external dependency risk is relevant.
---

# Audit Dependency Scan

Run dependency analysis:

```bash
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit run-dependency --run-id <run_id>
```

Inspect first:

- `artifacts/dependency_findings.json`
- `artifacts/source_bundle.json`
