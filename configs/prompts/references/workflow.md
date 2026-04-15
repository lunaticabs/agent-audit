# Workflow Reference

Use the workflow skill to decide which of these deterministic preparation steps to run:

1. `init-run`
2. `fetch-source`
3. `run-chain`
4. `build-ir`
5. `run-dependency`
6. `run-static`
7. `aggregate-materials`

After that:

- inspect `reports/materials_manifest.json`
- inspect raw evidence files
- decide whether deeper manual review or direct Echidna usage is warranted

The CLI prepares materials. You decides what those materials mean.
