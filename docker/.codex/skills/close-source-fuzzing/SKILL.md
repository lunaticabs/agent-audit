---
name: close-source-fuzzing
description: Use when auditing closed-source, unverified, or bytecode-only EVM contracts and when fuzzing deployed contracts for economic-security issues with Foundry or Echidna.
---

# Closed-Source Economic Fuzzing

For closed-source EVM targets, economic fuzzing is a primary audit activity, not an optional follow-up. Use it early to search for concrete loss signals before concluding that a bytecode-only target has no confirmed economic vulnerability.

Start from:

- `runs/<run_id>/artifacts/bytecode_targets.json`
- `runs/<run_id>/artifacts/tooling_manifest.json`
- `runs/<run_id>/foundry_project/build_manifest.json`
- `runs/<run_id>/echidna_project/build_manifest.json`
- `runs/<run_id>/artifacts/proxy_checks.json` when present

If RPC is missing, record the fuzzing attempt as blocked by missing RPC. Do not claim that fork fuzzing was performed.

## Priority Properties

For each high-risk closed-source target, fuzz low-level calls and domain-specific call shapes for economic symptoms:

- `sender.balance` or the harness caller's native ETH balance increases unexpectedly after a call.
- ERC20/ERC721/ERC1155 balances owned by the sender or attacker increase unexpectedly.
- debt, shares, collateral, reserve accounting, or claimable amounts move in the attacker's favor without matching payment.
- privileged or approval-like state changes are reachable by an unprivileged sender.
- target-held native ETH or token balances decrease while the attacker balance increases.
- repeated call sequences produce profit, bypass fees, bypass cooldowns, or release locked assets.

Treat a balance increase as a lead, not automatically a finding. Confirm it with a minimized calldata/value/sequence, a trace, and an explanation of why the gain is unauthorized.

## Foundry First Pass

Prefer Foundry for the first smoke fuzz because it is easy to run fork tests, use low-level calls, control senders, and capture traces.

Use `runs/<run_id>/foundry_project` even when source is unavailable. Put harnesses under `test/` or `runs/<run_id>/sources/forge/`.

Recommended harness shape:

- select target addresses from `bytecode_targets.json`;
- fork with `AGENT_AUDIT_RPC_URL`;
- create one or more unprivileged senders with bounded ETH;
- fuzz selector, calldata tail, msg.value, sender, and call sequence length;
- call deployed targets via `target.call{value: boundedValue}(data)`;
- compare pre/post native and token balances;
- persist counterexamples, raw `forge test` output, and traces under `runs/<run_id>/artifacts/`.

Use `forge test -vvvv` for confirmed counterexamples. Save at least:

- `runs/<run_id>/artifacts/closed_source_fuzzing_plan.json`
- `runs/<run_id>/artifacts/closed_source_fuzzing_output.txt`
- `runs/<run_id>/artifacts/closed_source_fuzzing_findings.json`
- harness source under `runs/<run_id>/sources/forge/` or `runs/<run_id>/foundry_project/test/`

## Echidna Follow-Up

Use Echidna when a property benefits from longer campaigns, call-sequence exploration, or corpus shrinking.

Use `runs/<run_id>/echidna_project` even when source is unavailable. Put harnesses under `test/` or `runs/<run_id>/sources/echidna/`.

Good Echidna properties for closed-source targets:

- no unprivileged call sequence increases attacker net worth;
- no repeated sequence drains target-held ETH/tokens;
- no sequence grants privileged roles or approvals to the attacker;
- no sequence reduces attacker debt or unlocks collateral without equivalent payment.

Save raw Echidna output and any minimized sequence under `runs/<run_id>/artifacts/`, then rerun `agent-audit aggregate-materials --run-id <run_id>` if those outputs should appear in the material manifest.

## Reporting

Do not report unsupported fuzzing claims. A finding needs artifact support: harness source, exact command, raw output, minimized calldata/sequence, relevant trace, and the balance/accounting delta. If fuzzing runs but finds no counterexample, include it as evidence scope and keep limitations explicit.
