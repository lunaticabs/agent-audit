---
name: workspace
description: Create a new run workspace for a contract address. Use when you need a fresh runs/<run_id>/ directory before preparing audit materials.
---

# Audit Workspace

Initialize a run workspace:

```bash
cargo run --bin agent-audit -- init-run --chain <chain> --address <address>
```

What it does:

- Creates `runs/<run_id>/`
- Creates `input/`, `artifacts/`, `reports/`, and `logs/`
- Writes `input/request.json`
- Writes `input/run_meta.json`

It does not fetch source or run any analyzer.
