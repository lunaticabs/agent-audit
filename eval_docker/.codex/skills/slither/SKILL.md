---
name: slither
description: Run Slither directly against the benchmark-provided EVMbench audit repository.
---

# EVMbench Slither

Run Slither from `CODEX_WORKDIR` / `AUDIT_DIR`, not from a production
`runs/<run_id>/slither_project`.

Start by identifying the local project shape:

```bash
pwd
find . -maxdepth 4 -type f \( -name 'foundry.toml' -o -name 'hardhat.config.*' -o -name 'package.json' -o -name 'remappings.txt' -o -name '*.sol' \)
```

Common local workflows:

```bash
slither .
```

```bash
slither path/to/Contract.sol
```

If a compiler version is specified by the local project, use `solc-select`
before running Slither:

```bash
solc-select use <solc_version> --always-install
slither .
```

Save useful raw output under `LOGS_DIR` or a local intermediate directory:

```bash
mkdir -p "${LOGS_DIR:-logs}"
slither . --json "${LOGS_DIR:-logs}/slither.json"
```

Audit guidance:

- Treat Slither output as triage; manually confirm every finding in source.
- Cite direct repository file and line references in `submission/audit.md`.
- Local `runs/` or `artifacts/` files are allowed for notes and raw output.
- Do not run `agent-audit prepare-slither`, `prepare-tooling`, or
  `aggregate-materials`; those are production pipeline steps.

Official docs:

- https://github.com/crytic/slither/blob/master/docs/src/Usage.md
