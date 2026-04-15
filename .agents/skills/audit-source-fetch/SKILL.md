---
name: audit-source-fetch
description: Fetch verified source and dependency source bundles for an existing run. Use when the run workspace exists and source materials are still missing.
---

# Audit Source Fetch

Fetch verified source:

```bash
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit fetch-source --run-id <run_id>
```

Inspect first:

- `artifacts/source_bundle.json`
- `artifacts/source_provider_response.json`
- `sources/`
