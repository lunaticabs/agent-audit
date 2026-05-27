---
name: done
description: Finalize an EVMbench Detect report in the benchmark submission path.
---

# EVMbench Done

EVMbench Detect is complete only when the official report exists and is
non-empty:

```bash
test -s "${SUBMISSION_DIR:-submission}/audit.md" || test -s submission/audit.md
```

If `submission/audit.md` exists relative to the audit repository but
`${SUBMISSION_DIR}/audit.md` is empty, copy it to the official location:

```bash
mkdir -p "${SUBMISSION_DIR:-submission}"
cp submission/audit.md "${SUBMISSION_DIR:-submission}/audit.md"
```

Optional local `runs/`, `artifacts/`, or log files may remain as supporting
material, but EVMbench consumes only `audit.md`.

Do not run `agent-audit sync-run`. There is no MongoDB sync step in this eval
runtime.
