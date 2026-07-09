---
name: heimdall
description: Use Heimdall bytecode analysis artifacts and commands for closed-source EVM audits inside the containerized audit runtime.
---

# Heimdall Bytecode Review

Prepared artifacts:

```text
runs/<run_id>/artifacts/bytecode.json
runs/<run_id>/artifacts/runtime_bytecode.hex
runs/<run_id>/artifacts/heimdall_manifest.json
runs/<run_id>/artifacts/heimdall_decompiled.sol
runs/<run_id>/artifacts/heimdall_disassembly.txt
runs/<run_id>/artifacts/heimdall_cfg.dot
runs/<run_id>/artifacts/selector_index.json
runs/<run_id>/artifacts/storage_probe_plan.json
```

Regenerate Heimdall materials:

```bash
agent-audit prepare-heimdall --run-id <run_id>
```

Useful direct commands:

```bash
heimdall decompile runs/<run_id>/artifacts/runtime_bytecode.hex --output print --include-sol --skip-resolving --default > runs/<run_id>/artifacts/heimdall_decompiled_manual.sol
heimdall disassemble runs/<run_id>/artifacts/runtime_bytecode.hex --output print --default > runs/<run_id>/artifacts/heimdall_disassembly_manual.txt
heimdall cfg runs/<run_id>/artifacts/runtime_bytecode.hex --output print --default > runs/<run_id>/artifacts/heimdall_cfg_manual.dot
```

Rules:

- Save all direct Heimdall output under `runs/<run_id>/artifacts/`.
- Treat decompiled pseudo-code as a lead, not as verified Solidity source.
- Confirm findings with bytecode, selector evidence, storage reads, RPC calls, traces, or fork reproduction.
- Rerun `agent-audit aggregate-materials --run-id <run_id>` after saving extra artifacts if you want them listed in the materials manifest.
