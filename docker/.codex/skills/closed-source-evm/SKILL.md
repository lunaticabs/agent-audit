---
name: closed-source-evm
description: Audit unverified, closed-source, bytecode-only EVM contracts and closed proxy implementations, with priority on fork-backed economic-security fuzzing.
---

# Closed-Source EVM Audit

Use this when the target contract, a proxy implementation, or a dependency is unverified, closed source, bytecode-only, or listed in `artifacts/bytecode_targets.json`.

Closed-source review should prioritize economic-security evidence. Do not conclude from selector matching, known-project similarity, or decompiler inspection alone when fork-backed fuzzing can test concrete balance and accounting properties.

## Audit Goals

Focus first on whether an unprivileged caller can create economic gain or force target loss:

- native ETH balance increases for the sender, harness caller, or attacker account after a call or call sequence;
- ERC20/ERC721/ERC1155 balances increase unexpectedly for the attacker;
- target-held native ETH or token balances decrease while the attacker balance increases;
- debt, shares, collateral, reserve accounting, or claimable amounts move in the attacker's favor without equivalent payment;
- approvals, roles, privileged flags, or configuration state become reachable by an unprivileged caller;
- repeated calls bypass fees, cooldowns, access checks, flash-loan constraints, or withdrawal limits.

Also review bytecode-level safety surfaces:

- upgradeability and proxy slot behavior;
- `delegatecall`, `callcode`, fallback dispatch, and library-call targets;
- access-control gates and privileged selectors;
- external calls and value transfers;
- storage writes and authorization-sensitive state changes.

Treat any balance increase as a lead, not automatically a finding. Confirm it with minimized calldata/value/sequence, trace evidence, and an explanation of why the gain is unauthorized.

## Audit Method

Start by reading:

- `runs/<run_id>/artifacts/source_bundle.json`
- `runs/<run_id>/artifacts/bytecode_targets.json`
- `runs/<run_id>/artifacts/tooling_manifest.json`
- `runs/<run_id>/foundry_project/build_manifest.json`
- `runs/<run_id>/echidna_project/build_manifest.json`
- `runs/<run_id>/artifacts/heimdall/build_manifest.json`
- `runs/<run_id>/artifacts/proxy_checks.json`
- `runs/<run_id>/reports/materials_manifest.json`

For each high-risk item in `bytecode_targets.json`, inspect:

- `role`
- `origin`
- `origin_evidence`
- `source_availability`
- `bytecode_status`
- `bytecode_artifact`

If RPC is unavailable or `bytecode_status` is `rpc_not_configured`, do not claim fork fuzzing or bytecode review was performed. Record the audit as blocked by missing RPC for those steps.

General fuzzing workflow:

1. Recover selectors from bytecode, disassembly, Heimdall output, known interfaces, proxy metadata, and 4byte candidates.
2. Classify selectors by risk: withdrawals, claims, redeeming, borrowing, liquidation, flash loans, token movement, approvals, admin/config, and accounting updates.
3. Build a fork-backed harness with one or more unprivileged attacker senders.
4. Fuzz selector, calldata tail, `msg.value`, sender, and call sequence length.
5. Compare pre/post native ETH, relevant token balances, target balances, debt/share/collateral/accounting values, and role/approval state.
6. Minimize any counterexample, rerun with traces, and save exact command output and harness source as artifacts.
7. If no counterexample is found, report the fuzz scope and limitations instead of overstating safety.

Use both selector-aware fuzzing and blind low-level-call fuzzing. 4byte results are hints, not verified ABI: the same selector can map to multiple signatures or no useful signature.

## Mandatory Completion Gate

Before writing `reports/final_report.json`, decide whether closed-source fuzzing was required:

- Required when `bytecode_targets.json` contains a source-unavailable target, implementation, or dependency and RPC is configured.
- Blocked when RPC is missing, runtime bytecode is missing, the fork cannot be created, or the fuzz harness cannot compile/run.
- Optional only when there is no closed-source bytecode target in scope.

If fuzzing is required, do not write `status: "completed"`, `result: "no_confirmed_findings"`, or equivalent final language unless the report cites fuzzing artifacts from this run, including a harness source file, exact command/output, and either findings or a no-counterexample result with scope limits.

If fuzzing is required but not completed, the final report must use an incomplete or inconclusive result such as `incomplete_missing_closed_source_fuzzing` and must explain the blocker. Do not bury skipped fuzzing as a normal limitation while still presenting the audit as complete.

## Tools And Commands

Use the CLI-prepared workspaces. If needed, rerun:

```bash
agent-audit prepare-tooling --run-id <run_id>
```

Foundry is the preferred first pass because fork tests, low-level calls, sender control, and traces are direct:

```bash
cd runs/<run_id>/foundry_project
forge test --fork-url "$AGENT_AUDIT_RPC_URL" -vvvv
```

Put Foundry harnesses under `runs/<run_id>/foundry_project/test/` or save copies under `runs/<run_id>/sources/forge/`. A closed-source harness should call deployed targets with low-level calls:

```solidity
(bool ok, bytes memory ret) = target.call{value: boundedValue}(data);
```

Use `vm.createSelectFork`, `vm.deal`, `vm.prank` or `vm.startPrank`, and `bound` to control sender, value, fork state, and fuzz ranges. Save:

- `runs/<run_id>/artifacts/closed_source_fuzzing_plan.json`
- `runs/<run_id>/artifacts/closed_source_fuzzing_output.txt`
- `runs/<run_id>/artifacts/closed_source_fuzzing_findings.json`
- the harness source

Echidna is useful for longer property campaigns and sequence exploration:

```bash
cd runs/<run_id>/echidna_project
ECHIDNA_RPC_URL="$AGENT_AUDIT_RPC_URL" echidna .
```

Put Echidna harnesses under `runs/<run_id>/echidna_project/test/` or `runs/<run_id>/sources/echidna/`. Good properties include: no unprivileged sequence increases attacker net worth, drains target assets, grants roles/approvals, reduces debt, or unlocks collateral without matching payment.

Use Heimdall for bytecode orientation and selector/control-flow evidence. Do not create Heimdall directories or redirect output by hand; run prepared scripts:

```bash
bash runs/<run_id>/artifacts/heimdall/<chain>/<address>/decompile/run.sh
bash runs/<run_id>/artifacts/heimdall/<chain>/<address>/disassemble/run.sh
bash runs/<run_id>/artifacts/heimdall/<chain>/<address>/cfg/run.sh
```

Use `cast` for direct probes and trace-oriented checks:

```bash
cast call <target> "balanceOf(address)(uint256)" <account> --rpc-url "$AGENT_AUDIT_RPC_URL"
cast 4byte <selector>
```

Use 4byte as selector enrichment when local decoding is incomplete:

```bash
curl -s "https://www.4byte.directory/api/v1/signatures/?hex_signature=0xa9059cbb"
```

Save selector candidates under `runs/<run_id>/artifacts/selector_signature_candidates.json`; do not treat them as verified ABI.

After running tools, refresh the material manifest when useful:

```bash
agent-audit aggregate-materials --run-id <run_id>
```

## Reporting

Treat Heimdall decompiler output as heuristic evidence. Do not treat pseudocode, reconstructed Solidity/Yul, selector names, or 4byte signatures as verified source.

A confirmed finding needs concrete artifact support: harness source, exact command, raw output, minimized calldata/sequence, relevant trace, and the balance/accounting delta. If fuzzing is skipped, blocked, or inconclusive, state that explicitly in limitations or unconfirmed hypotheses; do not present unsupported no-risk conclusions as findings or as complete coverage.
