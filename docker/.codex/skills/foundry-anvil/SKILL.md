---
name: foundry-anvil
description: Use Foundry's anvil tool for local nodes and forked-chain reproduction inside the containerized audit runtime.
---

# Foundry Anvil

```bash
anvil
```

RPC source:

- When fork mode is needed, default to `AGENT_AUDIT_RPC_URL` from the container environment.
- Only override it when you intentionally need a different chain or provider.

Common audit workflows:

- start a local dev node:

```bash
anvil
```

- fork a live chain:

```bash
anvil --fork-url "$AGENT_AUDIT_RPC_URL"
```

- enable automatic impersonation:

```bash
anvil --fork-url "$AGENT_AUDIT_RPC_URL" --auto-impersonate
```

Default artifact convention for a current run:

```text
runs/<run_id>/artifacts/anvil_plan.json
runs/<run_id>/artifacts/anvil_output.txt
runs/<run_id>/artifacts/anvil_findings.json
```

Audit guidance:

- Use Anvil when you need reproducible local execution against live state.
- Prefer running Anvil in a separate terminal/session, then use `cast` or `forge` against that local RPC.
- Fork mode is the right place to test exploit reproduction, privileged calls, or balance-sensitive paths.
- Default Anvil accounts are public test accounts only; never treat them as secure keys.
- Save the launch command, fork source, and intent in `artifacts/anvil_plan.json`.
- Save startup output or relevant logs in `artifacts/anvil_output.txt`.
- If you summarize any environment assumptions or reproduction notes, save them in `artifacts/anvil_findings.json`.
- Rerun `agent-audit aggregate-materials --run-id <run_id>` if you want the manifest to list these optional artifacts.

Official docs:

- Overview: https://getfoundry.sh/anvil
- Forking with Cast and Anvil: https://getfoundry.sh/guides/forking-mainnet-with-cast-anvil/
