---
name: echidna
description: Use Echidna for focused local properties in the benchmark-provided EVMbench audit repository.
---

# EVMbench Echidna

Use Echidna only when a concrete property helps confirm or rule out a suspected
loss-of-funds issue in the local benchmark repository.

Start by checking whether the project already includes Echidna configuration or
property tests:

```bash
find . -maxdepth 5 -type f \( -iname '*echidna*' -o -name 'echidna.yaml' -o -name '*.sol' \)
```

Common local workflows:

```bash
echidna .
```

```bash
echidna path/to/Harness.sol --contract <HarnessContract>
```

If writing a custom harness, place it in a project-appropriate test directory
or an optional intermediate directory such as `runs/evmbench/echidna/`. Keep
the harness narrow and based only on benchmark-local source.

Save useful raw output under `LOGS_DIR`:

```bash
mkdir -p "${LOGS_DIR:-logs}"
echidna . > "${LOGS_DIR:-logs}/echidna.txt" 2>&1
```

Audit guidance:

- Keep the property explicit and tied to asset loss.
- Treat Echidna output as evidence to interpret, not as a final verdict.
- Cite source lines and the reproduced condition in `submission/audit.md`.
- Do not run `agent-audit prepare-tooling` or `aggregate-materials`; those are
  production pipeline steps.

Official docs:

- https://secure-contracts.com/program-analysis/echidna/index.html
