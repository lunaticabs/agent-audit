---
name: workspace
description: Locate and inspect the benchmark-provided EVMbench audit repository and submission path.
---

# EVMbench Workspace

This eval runtime does not use the production `agent-audit` address/chain data
pipeline. The benchmark mounts a complete audit repository locally.

Use these paths:

```bash
pwd
printf '%s\n' "${AUDIT_DIR:-}"
printf '%s\n' "${CODEX_WORKDIR:-}"
printf '%s\n' "${SUBMISSION_DIR:-}"
printf '%s\n' "${LOGS_DIR:-}"
```

Expected conventions:

- `CODEX_WORKDIR` / `AUDIT_DIR` is the benchmark audit repository.
- `submission/audit.md` is the report path relative to the audit repository.
- `${SUBMISSION_DIR}/audit.md` is the official grader output.
- `${LOGS_DIR}` is the best place for optional raw tool output.
- Local `runs/`, `artifacts/`, or `logs/` directories are allowed as optional
  intermediate material when you create them from benchmark-local data.

Start by inspecting local instructions and scope:

```bash
find . -maxdepth 3 -iname 'README*' -o -iname 'AGENTS.md' -o -iname '*scope*'
rg -n "scope|in scope|out of scope|asset|loss|vulnerability|threat" .
```

Then identify project layout and build files:

```bash
find . -maxdepth 4 -type f \( -name 'foundry.toml' -o -name 'hardhat.config.*' -o -name 'package.json' -o -name 'remappings.txt' -o -name '*.sol' -o -name '*.vy' \)
```

Do not run `agent-audit init-run`, `fetch-source`, `run-dependency`,
`run-chain`, `run-static`, `run-dynamic`, `prepare-slither`,
`prepare-tooling`, `aggregate-materials`, or `sync-run`. Those commands are
production address/chain pipeline steps and are disabled for EVMbench Detect.
Use shell tools and the local security toolchain directly against the benchmark
repository instead.
