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
- decide whether use tools like: Slither, Echidna, Forge, Cast, or Anvil
- if you run direct tools, save their artifacts under the same `runs/<run_id>/artifacts/` tree
- rerun `aggregate-materials` if you want the manifest to list those optional artifacts

Repository-side findings, when present, live in `artifacts/dependency_findings.json`.

Finally, if you think you have figured out the all real vulnerabilities, or the contract is safe, give a report in JSON, and save it in `runs/<run_id>/reprots/final_reprot.json`.
