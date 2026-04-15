---
name: foundry-chisel
description: Use Foundry's chisel REPL for quick Solidity experiments, decoding, and small proof-of-understanding checks. Use when you want to prototype Solidity snippets interactively without committing a file yet.
---

# Foundry Chisel

Use `chisel` through the repository devShell:

```bash
nix develop .#default -c chisel
```

Common audit workflows:

- start the REPL:

```bash
nix develop .#default -c chisel
```

- inspect available commands from inside the REPL:

```text
!help
```

- inspect the generated Solidity source from the current REPL session:

```text
!source
```

Default artifact convention for a current run:

```text
runs/<run_id>/artifacts/chisel_plan.json
runs/<run_id>/artifacts/chisel_output.txt
runs/<run_id>/artifacts/chisel_findings.json
runs/<run_id>/sources/chisel/
```

Audit guidance:

- Use Chisel for quick Solidity sanity checks before creating a full harness.
- It is best for small experiments, type checks, arithmetic, ABI intuition, and cheatcode-aware snippets.
- Save the purpose of the session in `artifacts/chisel_plan.json`.
- Save useful REPL output in `artifacts/chisel_output.txt`.
- If you extract a reusable snippet or conclusion, save it in `artifacts/chisel_findings.json`.
- If the experiment becomes important evidence, move the snippet or generated source into `sources/chisel/`.
- Rerun `agent-audit aggregate-materials --run-id <run_id>` if you want the manifest to list these optional artifacts.

Official docs:

- Overview: https://getfoundry.sh/chisel/overview/
