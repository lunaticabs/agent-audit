---
name: foundry-cast
description: Use Foundry cast for local decoding and local-node interaction during EVMbench Detect.
---

# EVMbench Cast

Use `cast` directly from the benchmark audit repository for local-only audit
work:

- ABI and calldata encoding or decoding
- selector inspection
- local Anvil interaction
- read-only calls against a benchmark-provided local node, if one exists

Common local workflows:

```bash
cast 4byte-decode <selector_or_calldata>
```

```bash
cast calldata "transfer(address,uint256)" <to> <amount>
```

```bash
cast call <contract> "balanceOf(address)(uint256)" <arg> --rpc-url http://127.0.0.1:8545
```

Save useful raw output under `LOGS_DIR` or optional local `runs/` artifacts:

```bash
mkdir -p "${LOGS_DIR:-logs}"
cast 4byte-decode <selector_or_calldata> > "${LOGS_DIR:-logs}/cast_decode.txt"
```

Audit guidance:

- Do not default to `AGENT_AUDIT_RPC_URL`; EVMbench Detect source is local.
- Do not query real chain state unless the benchmark task explicitly provides
  and requires that RPC source.
- `cast send` is state-changing; use it only against a local Anvil node for
  reproduction.
- Do not run production `agent-audit` source or chain commands.

Official docs:

- https://getfoundry.sh/cast
