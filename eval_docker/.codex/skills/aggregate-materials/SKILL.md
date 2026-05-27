---
name: aggregate-materials
description: Organize optional local EVMbench audit notes and tool outputs without using the production pipeline.
---

# EVMbench Materials

For EVMbench Detect, materials are optional local working files. They can help
organize evidence, but the final benchmark output must be `submission/audit.md`.

Good local locations:

```text
${LOGS_DIR}/
runs/
artifacts/
logs/
```

Use benchmark-local data only:

- source files from `CODEX_WORKDIR` / `AUDIT_DIR`
- local build, test, static-analysis, fuzzing, or trace output
- notes that cite repository file and line references

Do not run `agent-audit aggregate-materials`. That command aggregates
production `runs/<run_id>` pipeline artifacts and is not part of EVMbench
Detect.

If you create a summary manifest yourself, keep it simple and local, for
example:

```bash
mkdir -p "${LOGS_DIR:-logs}"
find . -maxdepth 3 -type f \( -path './runs/*' -o -path './artifacts/*' -o -path './logs/*' \) \
  > "${LOGS_DIR:-logs}/local_materials.txt"
```
