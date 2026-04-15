---
name: audit-slither
description: Use Slither directly from the Nix devShell when verified source is available and you want static-analysis signals on a chosen target file.
---

# Audit Slither

Run Slither directly:

```bash
nix develop .#default -c slither <target_path>
```

Choose the target from prepared materials first:

- `artifacts/source_bundle.json`
- `ir/contracts.json`
- `analysis_target` inside the source bundle when present

Default artifact convention for a current run:

```text
runs/<run_id>/artifacts/static_plan.json
runs/<run_id>/artifacts/slither_raw.json
runs/<run_id>/artifacts/static_findings.json
```

Suggested pattern:

1. Save the exact command and target choice in `artifacts/static_plan.json`.
2. Save raw Slither output in `artifacts/slither_raw.json`.
3. If you manually normalize or summarize candidate findings, save that in `artifacts/static_findings.json`.

Offical docs:

https://github.com/crytic/slither/blob/master/docs/src/Usage.md
