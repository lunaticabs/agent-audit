---
name: echidna
description: Use Echidna when a concrete contract property merits fuzzing inside the containerized audit runtime.
---

# Audit Echidna

If you need a local fork, impersonation, or low-level chain interaction first, also use `$foundry-anvil` and `$foundry-cast`.

Container variant:

- Use the installed `agent-audit` binary directly.

Run Echidna from `runs/<run_id>/echidna_project/build_manifest.json`'s `preferred_working_dir`.

Use this workflow:

1. Ensure `runs/<run_id>/echidna_project/build_manifest.json` exists. Normally `$workspace` already prepares it. If needed, rerun `agent-audit prepare-tooling --run-id <run_id>`.
2. `cd runs/<run_id>/echidna_project`
3. Put custom harnesses under `runs/<run_id>/sources/echidna/` or `runs/<run_id>/echidna_project/test/`.

```bash
cd runs/<run_id>/echidna_project && echidna .
```

Default artifact convention for a current run:

```text
runs/<run_id>/artifacts/echidna_plan.json
runs/<run_id>/artifacts/echidna_output.txt
runs/<run_id>/artifacts/echidna_findings.json
runs/<run_id>/sources/echidna/
```

Guidelines:

- Keep the target narrow.
- Make the property explicit.
- Save the exact command, target, and property in `runs/<run_id>/artifacts/echidna_plan.json`.
- Save raw Echidna output in `runs/<run_id>/artifacts/echidna_output.txt`.
- If you summarize or normalize candidate findings, save them in `runs/<run_id>/artifacts/echidna_findings.json`.
- Save any harness or helper source under `runs/<run_id>/sources/echidna/`.
- Rerun `agent-audit aggregate-materials --run-id <run_id>` if you want the manifest to list these optional artifacts.
- Treat Echidna output as evidence material to interpret, not as a final verdict.

Official docs:

- https://secure-contracts.com/program-analysis/echidna/index.html
