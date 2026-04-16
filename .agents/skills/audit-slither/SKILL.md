---
name: audit-slither
description: Use Slither directly from the Nix devShell when verified source is available and you want static-analysis signals on a chosen target file.
---

# Audit Slither

`fetch-source` normally prepares the Slither workspace for you.

If you need to rebuild it manually:

```bash
UV_CACHE_DIR=/tmp/uv-cache uv run agent-audit prepare-slither --run-id <run_id>
```

Then run Slither from the reconstructed project root:

```bash
nix develop .#default -c zsh -lc 'cd runs/<run_id>/slither_project && slither <prepared_target_path>'
```

Choose the target from prepared materials first:

- `artifacts/source_bundle.json`
- `ir/contracts.json`
- `analysis_target` inside the source bundle when present
- `slither_project/build_manifest.json`

The preparer also writes:

- `slither_project/remappings.txt`
- `slither_project/slither_inputs.json`

Read `slither_project/build_manifest.json` for:

- `preferred_working_dir`
- `preferred_target`
- `remappings`
- `solc_args`
- `solc_select`

Before running Slither, check `solc_select` in the manifest:

- if `is_installed` is `true`, switch to the requested version inside the devShell
- if `is_installed` is `false`, follow `recommended_action` before invoking Slither

In practice the stable command shape is:

```bash
nix develop .#default -c zsh -lc 'cd runs/<run_id>/slither_project && solc-select use <solc_version> && slither <preferred_target> --solc-working-dir . --solc-remaps "<remaps joined by spaces>" --solc-args "<solc_args>"'
```

Default artifact convention for a current run:

```text
runs/<run_id>/artifacts/static_plan.json
runs/<run_id>/artifacts/slither_raw.json
runs/<run_id>/artifacts/static_findings.json
```

Suggested pattern:

1. Inspect `slither_project/build_manifest.json`.
2. If it is missing or stale, rerun `prepare-slither`.
3. Save the exact command and target choice in `artifacts/static_plan.json`.
4. Save raw Slither output in `artifacts/slither_raw.json`.
5. If you manually normalize or summarize candidate findings, save that in `artifacts/static_findings.json`.

Offical docs:

https://github.com/crytic/slither/blob/master/docs/src/Usage.md
