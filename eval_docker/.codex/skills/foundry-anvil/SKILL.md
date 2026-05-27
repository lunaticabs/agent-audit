---
name: foundry-anvil
description: Use Foundry anvil for local-only EVM reproduction during EVMbench Detect.
---

# EVMbench Anvil

Use Anvil as a local dev node when it helps reproduce a benchmark-local issue.

Common local workflows:

```bash
anvil
```

```bash
anvil --auto-impersonate
```

Then interact with it through Forge or Cast:

```bash
cast block-number --rpc-url http://127.0.0.1:8545
```

Save useful output under `LOGS_DIR`:

```bash
mkdir -p "${LOGS_DIR:-logs}"
anvil > "${LOGS_DIR:-logs}/anvil.txt" 2>&1
```

Audit guidance:

- Prefer local execution from the benchmark repository.
- Do not default to `AGENT_AUDIT_RPC_URL` or real chain forks.
- Only use fork mode if the benchmark repository explicitly provides and
  requires that RPC source.
- Default Anvil keys are public local test keys only.

Official docs:

- https://getfoundry.sh/anvil
