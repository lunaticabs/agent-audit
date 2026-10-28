---
name: evm-audit
description: Use when asked to audit, review, or find vulnerabilities in an EVM contract or address, including unverified or bytecode-only targets.
---

# EVM audit

The task message contains the target and scope; the runner also stores the same
text in `input/prompt.txt` for the run record. Work in the current run directory
and choose your own approach.

## Output

- Write `reports/final_report.json` before finishing.
- Save raw evidence under `artifacts/` and fetched source under `sources/`.
- A confirmed finding must cite artifact paths. Unsupported claims are hypotheses.
- The runner persists this directory automatically. Do not look for an archive command.
- This is a one-shot headless run. Record blockers instead of waiting for an answer.

## Non-obvious points

- Source verification and proxy status are not guaranteed. Check both on-chain
  before trusting the source at the prompted address.
- Resolve the implementation of a proxy before auditing it.
- If source is unavailable, audit the bytecode and test economic properties on
  a fork rather than treating the target as out of scope.

## References

- `references/tools.md` — environment variables, Etherscan v2, RPC, Foundry,
  Slither, Echidna, and Heimdall commands. Read before using a tool or resolving
  on-chain data.
- `references/chains.md` — supported chain aliases and IDs. Read when resolving
  the network.
- `references/report.md` — report shape and evidence examples. Read before
  writing the report.
