---
name: ir-builder
description: Build lightweight source-derived IR for an existing run. Use when source fetch has already completed and you want structural materials for review.
---

# Audit IR Builder

Build IR:

```bash
uv run agent-audit build-ir --run-id <run_id>
```

Inspect first:

- `ir/contracts.json`
- `ir/functions.json`
- `ir/privilege_matrix.json`
