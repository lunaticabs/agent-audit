---
name: foundry-cast
description: Use Foundry's cast tool for RPC queries, contract calls, calldata decoding, and local-node interaction. Use when you need chain data, storage reads, ABI encoding/decoding, or controlled local transactions.
---

# Foundry Cast

Use `cast` through the repository devShell:

```bash
nix develop .#default -c cast <subcommand> ...
```

RPC source:

- By default, use the repository-local `.env` value in `AGENT_AUDIT_RPC_URL`.
- Direct shell commands need the current shell to load `.env` first so `"$AGENT_AUDIT_RPC_URL"` expands correctly.
- If the task needs a different chain or a local fork, explicitly say so and override it on purpose.

Common audit workflows:

- block number:

```bash
nix develop .#default -c bash -lc 'source .env && cast block-number --rpc-url "$AGENT_AUDIT_RPC_URL"'
```

- deployed bytecode:

```bash
nix develop .#default -c bash -lc 'source .env && cast code <address> --rpc-url "$AGENT_AUDIT_RPC_URL"'
```

- storage slot:

```bash
nix develop .#default -c bash -lc 'source .env && cast storage <address> <slot> --rpc-url "$AGENT_AUDIT_RPC_URL"'
```

- read-only contract call:

```bash
nix develop .#default -c bash -lc 'source .env && cast call <contract> "balanceOf(address)(uint256)" <arg> --rpc-url "$AGENT_AUDIT_RPC_URL"'
```

- decode selectors or calldata:

```bash
nix develop .#default -c cast 4byte-decode <selector_or_calldata>
```

- local-node RPC interaction:

```bash
nix develop .#default -c cast rpc <method> <params...>
```

Default artifact convention for a current run:

```text
runs/<run_id>/artifacts/cast_plan.json
runs/<run_id>/artifacts/cast_<purpose>.txt
runs/<run_id>/artifacts/cast_findings.json
```

Audit guidance:

- Prefer `cast call`, `cast code`, and `cast storage` for non-destructive inspection.
- Use `cast rpc` when interacting with Anvil-specific methods such as impersonation.
- `cast send` is state-changing; only use it on local forks or when the user explicitly asks.
- Keep exact ABI signatures in the command so the interaction is auditable.
- Save the exact command, ABI signature, and purpose in `artifacts/cast_plan.json`.
- Save raw output in a purpose-specific file such as `artifacts/cast_storage.txt` or `artifacts/cast_call.txt`.
- If you summarize notable observations, save them in `artifacts/cast_findings.json`.
- Rerun `agent-audit aggregate-materials --run-id <run_id>` if you want the manifest to list these optional artifacts.

Official docs:

- Overview: https://getfoundry.sh/cast
- Forking with Cast and Anvil: https://getfoundry.sh/guides/forking-mainnet-with-cast-anvil/
