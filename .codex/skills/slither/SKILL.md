---
name: slither
description: Use Slither directly in the current shell when verified source is available
---

Run Slither with the CLI-generated `preferred_working_dir` and `preferred_target` from `slither_project/build_manifest.json`.

Use this workflow:

1. Ensure `slither_project/build_manifest.json` exists. If it does not, run `cargo run --bin agent-audit -- prepare-slither --run-id <run_id>`.
2. Use `preferred_working_dir`, `preferred_target`, `solc_version`, `solc_args`, and `remappings`.
3. If you intentionally analyze a different target, switch to that target's source root and keep the target path relative to that root.

```bash
cd runs/<run_id>/<preferred_working_dir> && solc-select use <solc_version> && slither <preferred_target> --solc-working-dir . --solc-remaps "<remappings joined by spaces>" --solc-args "<solc_args>"
```

If `remappings` is empty, omit `--solc-remaps`.

Offical docs:

https://github.com/crytic/slither/blob/master/docs/src/Usage.md
