---
name: audit-chain-checks
description: Use direct cast commands for live-chain context such as deployed bytecode, storage slots, and proxy metadata. Use when the audit needs on-chain facts but the repository CLI should not make those calls for you.
---

# Audit Chain Checks

This skill is for direct `cast` usage.

Also use `$foundry-cast` when you need broader command coverage.

Common audit checks:

- deployed bytecode:

```bash
nix develop .#default -c cast code <address> --rpc-url <rpc_url>
```

- EIP-1967 implementation slot:

```bash
nix develop .#default -c cast storage <address> 0x360894A13BA1A3210667C828492DB98DCA3E2076CC3735A920A3CA505D382BBC --rpc-url <rpc_url>
```

- EIP-1967 admin slot:

```bash
nix develop .#default -c cast storage <address> 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103 --rpc-url <rpc_url>
```

- EIP-1967 beacon slot:

```bash
nix develop .#default -c cast storage <address> 0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50 --rpc-url <rpc_url>
```

Default artifact convention for a current run:

```text
runs/<run_id>/artifacts/chain_checks_plan.json
runs/<run_id>/artifacts/chain_checks_output.txt
runs/<run_id>/artifacts/chain_checks_findings.json
```

Guidelines:

- Save the exact command set and intent in `artifacts/chain_checks_plan.json`.
- Save raw output in `artifacts/chain_checks_output.txt`.
- If you summarize notable observations, save them in `artifacts/chain_checks_findings.json`.
- Rerun `agent-audit aggregate-materials --run-id <run_id>` if you want the manifest to list these optional artifacts.
- Prefer a few targeted checks over broad exploratory RPC traffic.
- Treat these results as evidence material, not as a final verdict.
