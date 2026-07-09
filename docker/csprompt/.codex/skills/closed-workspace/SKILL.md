---
name: closed-workspace
description: Create and fully prepare a closed-source bytecode audit run workspace inside the containerized audit runtime.
---

# Closed-Source Audit Workspace

Initialize and fully prepare a closed-source run workspace:

```bash
agent-audit init-run --source-kind closed-source --chain <chain> --address <address>
```

Boolean alias:

```bash
agent-audit init-run --closed-source --chain <chain> --address <address>
```

What it does:

- Creates `runs/<run_id>/`
- Writes `runs/<run_id>/input/request.json` with `source_kind=closed_source`
- Writes `runs/<run_id>/input/run_meta.json`
- Writes a closed-source `runs/<run_id>/artifacts/source_bundle.json` skip marker
- Fetches deployed runtime bytecode into `runs/<run_id>/artifacts/bytecode.json`
- Writes `runs/<run_id>/artifacts/runtime_bytecode.hex`
- Writes `runs/<run_id>/artifacts/selector_index.json`
- Writes `runs/<run_id>/artifacts/storage_probe_plan.json`
- Runs Heimdall and writes `runs/<run_id>/artifacts/heimdall_manifest.json`
- Writes `runs/<run_id>/reports/materials_manifest.json`

Inspect first:

- `runs/<run_id>/reports/materials_manifest.json`
- `runs/<run_id>/artifacts/bytecode.json`
- `runs/<run_id>/artifacts/heimdall_manifest.json`
- `runs/<run_id>/artifacts/selector_index.json`
- `runs/<run_id>/artifacts/storage_probe_plan.json`
