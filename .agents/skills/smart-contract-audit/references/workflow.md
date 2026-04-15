# Workflow Reference

The repository CLI prepares deterministic materials only.

Suggested order:

1. `init-run`
2. `fetch-source`
3. `build-ir`
4. `run-dependency`
5. `aggregate-materials`

After that:

- inspect `reports/materials_manifest.json`
- inspect raw evidence files
- decide whether direct tool usage such as chain checks, Slither, Echidna, Forge, Cast, or Anvil is warranted
- if you run direct tools, save their artifacts under the same `runs/<run_id>/artifacts/` tree
- rerun `aggregate-materials` if you want the manifest to list those optional artifacts

Repository-side findings, when present, live in `artifacts/dependency_findings.json`.
