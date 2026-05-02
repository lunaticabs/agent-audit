---
name: done
description: Sync an existing run's evidence into MongoDB from the containerized audit runtime.
---

# Audit Done

Container variant:

- Use the installed `agent-audit` binary directly.
- Do not use `cargo run`.

```bash
agent-audit sync-run --run-id <run_id>
```

Use this after writing:

- `runs/<run_id>/reports/final_report.json`
