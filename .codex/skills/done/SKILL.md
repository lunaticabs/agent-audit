---
name: done
description: Sync an existing run's evidence into MongoDB. Use when you have finished the audit for a run.
---

# Audit Done

```bash
uv run agent-audit sync-run --run-id <run_id>
```

Use this after writing:

- `runs/<run_id>/reports/final_report.json`
