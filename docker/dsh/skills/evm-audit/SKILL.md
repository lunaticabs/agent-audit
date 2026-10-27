---
name: evm-audit
description: Autonomous EVM smart-contract security audit for verified or unverified targets. Covers on-chain recon with Etherscan v2 and RPC, source review, proxy and dependency tracing, bytecode decompilation, fork tests, and economic-security fuzzing with Foundry, Echidna, Slither, and Heimdall.
whenToUse: Load at the start of every audit run, before choosing tools or writing findings.
---

# EVM audit skill

This is the main audit skill for the container. It is a toolbox and an evidence
standard, not a fixed workflow. You decide the order and depth of source review,
static analysis, on-chain recon, decompilation, fork testing, and fuzzing based
on what the target actually is.

## Workspace and output

You are in the run directory (`RUN_DIR`, also the shell's working directory).

- `input/prompt.txt` — the caller's full request, including target, chain, and
  any focus areas.
- `artifacts/` — raw evidence: API responses, command output, traces, harnesses,
  decompiled code, minimized calldata, fuzz corpora.
- `sources/` — fetched source code and dependencies, if you save them.
- `reports/final_report.json` — required structured report.
- `reports/audit.md` — optional human-readable report.

The entrypoint persists the whole run directory automatically after the audit
ends. Never call a separate archive/persistence command; none exists.

## How to work

1. Read `input/prompt.txt`. Identify the target address, chain, and any requested
   scope. If the chain alias is unfamiliar, resolve it with
   `references/chains.md`; do not guess a chain ID.
2. Establish what is deployed: source verification status, proxy status,
   implementation/beacon, compiler settings, and relevant token/role/timelock
   relationships.
3. Pick the cheapest evidence that can confirm or rule out a hypothesis. Source
   reading, `cast` state probes, and fork tests are usually faster than broad
   static-analysis reports. Use static analysis and fuzzing when they add
   evidence, not because a checklist says so.
4. Keep everything that supports a conclusion. Save raw JSON/output before
   summarizing it.
5. Write the report when you have enough evidence, or when a real blocker
   prevents further progress. Say which it is.

Do not stop at "no issues found" unless you can explain the coverage you
actually achieved. Missing source, missing RPC, rate limits, and failed tool
runs are limitations, not clean results.

Useful high-impact areas to keep in mind (not a mandatory checklist):

- access control and privileged state changes
- business logic and accounting invariants
- oracle/price manipulation and stale data
- flash-loan-funded attacks and single-transaction value extraction
- missing input validation and unchecked external calls
- arithmetic/precision errors and rounding
- reentrancy and callback ordering
- proxy, upgradeability, initializer, and storage-layout issues

## Reconnaissance

### Etherscan v2

Use the API directly. One key works across supported chains; pass `chainid`.

```bash
BASE="${ETHERSCAN_API_URL:-https://api.etherscan.io/v2/api}"
CID="<chain id>"
ADDR="<0x address>"

curl -sS --get "$BASE" \
  --data-urlencode "chainid=$CID" \
  --data-urlencode "module=contract" \
  --data-urlencode "action=getsourcecode" \
  --data-urlencode "address=$ADDR" \
  --data-urlencode "apikey=$ETHERSCAN_API_KEY" \
  | tee "artifacts/etherscan_getsourcecode.json" | jq .
```

`getsourcecode` returns `SourceCode`, `ABI`, `ContractName`,
`CompilerVersion`, optimization settings, `EVMVersion`, `Proxy`,
`Implementation`, and constructor arguments. Treat an empty `SourceCode` as
unverified. A proxy result can point at an implementation whose own
verification status is different.

Other useful actions:

```bash
# ABI and creation/bytecode
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=contract" --data-urlencode "action=getabi" \
  --data-urlencode "address=$ADDR" --data-urlencode "apikey=$ETHERSCAN_API_KEY"
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=contract" --data-urlencode "action=getbytecode" \
  --data-urlencode "address=$ADDR" --data-urlencode "apikey=$ETHERSCAN_API_KEY"

# Runtime code, storage, balance through the proxy module
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=proxy" --data-urlencode "action=eth_getCode" \
  --data-urlencode "address=$ADDR" --data-urlencode "tag=latest" \
  --data-urlencode "apikey=$ETHERSCAN_API_KEY"
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=proxy" --data-urlencode "action=eth_getStorageAt" \
  --data-urlencode "address=$ADDR" --data-urlencode "position=0x0" \
  --data-urlencode "tag=latest" --data-urlencode "apikey=$ETHERSCAN_API_KEY"

# History and logs
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=account" --data-urlencode "action=txlist" \
  --data-urlencode "address=$ADDR" --data-urlencode "startblock=0" \
  --data-urlencode "endblock=latest" --data-urlencode "sort=desc" \
  --data-urlencode "apikey=$ETHERSCAN_API_KEY"
```

Also useful: `contract/getcontractcreation`, `account/balance`,
`account/tokentx`, `logs/getLogs`, `proxy/eth_getTransactionCount`,
`proxy/eth_getBalance`, `block/getblocknobytime`.

Respect the free-tier rate limit (roughly five requests per second). Cache each
response under `artifacts/`. If the API says a chain is unsupported, use
`RPC_URL`/`cast` and the chain's official explorer; do not fabricate data.

### RPC and cheap state probes

`RPC_URL` is an archive-capable JSON-RPC endpoint for the audit network.

```bash
cast chain-id --rpc-url "$RPC_URL"
cast code "$ADDR" --rpc-url "$RPC_URL"
cast balance "$ADDR" --rpc-url "$RPC_URL"
cast call "$ADDR" "owner()(address)" --rpc-url "$RPC_URL"
cast storage "$ADDR" 0x0 --rpc-url "$RPC_URL"
```

Common proxy slots to read with `cast storage`:

- EIP-1967 implementation:
  `0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc`
- EIP-1967 admin:
  `0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103`
- EIP-1967 beacon:
  `0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50`

A minimal-proxy runtime prefix (`363d3d373d3d3d363d73...5af43d82803e903d91602b57fd5bf3`)
also points at an implementation. Verify any candidate implementation has code
before auditing it.

## Verified source

When Etherscan returns source:

1. Save the returned source bundle and compiler settings under `sources/`.
2. Install the matching compiler with `solc-select` (or let Foundry pick it):
   `solc-select install <version> && solc-select use <version>`.
3. Review the target, its inherited contracts, libraries, and privileged
   surfaces. Resolve dependencies yourself from the source, Etherscan, the
   project repository, or package registries.
4. Use `slither` for a fast call-graph/state overview and `forge` for executable
   checks. Do not treat raw analyzer output as a finding.

Slither example:

```bash
slither . --json artifacts/slither.json
```

For a flat set of sources, a minimal `foundry.toml` plus the vendored
`forge-std` is usually enough. `forge install` needs network; if it is
unavailable, link the image copy instead:

```bash
mkdir -p lib
ln -sfn /opt/agent-audit/vendor/forge-std lib/forge-std
```

## Unverified / closed-source targets

Do not treat "source is unavailable" as "not auditable". Establish what the
bytecode does and test economic properties on a fork.

1. Fetch runtime bytecode with `cast code` or Etherscan `eth_getCode` and save it.
2. Resolve the implementation behind proxies before decompiling. Save each
   address and its code separately.
3. Orient yourself with disassembly, control-flow recovery, and decompilation.
   Extract candidate selectors and compare them with known ABI fragments and
   4byte candidates.
4. Use the fork to test concrete properties: can an unprivileged caller move
   native/token value, mint/burn shares, change debt or collateral accounting,
   bypass a fee/cooldown/limit, escalate a role, or make the target lose assets?
5. Minimize any counterexample and rerun with traces. Save the exact command,
   harness source, output, and calldata sequence.

Heimdall examples (adapt flags if the installed version differs; check
`heimdall --help`):

```bash
mkdir -p artifacts/heimdall
heimdall disassemble "$ADDR" --rpc-url "$RPC_URL" --output artifacts/heimdall/disassemble
heimdall decompile "$ADDR" --rpc-url "$RPC_URL" --output artifacts/heimdall/decompile
heimdall cfg "$ADDR" --rpc-url "$RPC_URL" --output artifacts/heimdall/cfg
```

Selector lookup:

```bash
cast 4byte 0xa9059cbb
curl -sS "https://www.4byte.directory/api/v1/signatures/?hex_signature=0xa9059cbb" \
  | tee artifacts/4byte_a9059cbb.json | jq .
```

4byte is a hint source, not an ABI. The same selector can have multiple
signatures or none.

Foundry low-level harness sketch:

```solidity
(bool ok, bytes memory ret) = target.call{value: value}(data);
```

Use `vm.createSelectFork(vm.envString("RPC_URL"))`, `vm.deal`, `vm.prank`/
`vm.startPrank`, and `bound` for sender/value/sequence control. Save harnesses
under `artifacts/` or `test/` and keep the trace.

Echidna is useful for longer sequence and invariant campaigns:

```bash
ECHIDNA_RPC_URL="$RPC_URL" echidna . --contract <HarnessContract> --config echidna.yaml
```

Good properties are about economic safety, for example: no unprivileged
sequence increases the attacker's net value, drains target-held assets, grants a
role/approval, or moves debt/collateral accounting without matching payment.

## Tool catalog

| Tool | Use it for |
| --- | --- |
| `curl` + `jq` | Direct Etherscan v2, 4byte, explorer, and project APIs |
| `cast` | Calls, storage, bytecode, traces, selector lookup, fork probes |
| `forge` | Fork tests, low-level call harnesses, minimized reproductions |
| `anvil` | Local forks/snapshots when you need a persistent chain |
| `slither` | Static analysis, call graphs, detectors, source triage |
| `solc-select` | Selecting the compiler version declared by the target |
| `echidna` | Property-based and sequence fuzzing, especially on forks |
| `heimdall` | Bytecode disassembly, CFG recovery, decompilation |
| DSH `web_search`/`web_fetch` | Docs, explorer pages, advisories, context |
| `git`, `python3` | Repositories, scripts, data processing |
| DSH `subagent`/`workflow` | Optional parallel exploration of independent questions |

You also have normal shell access. Compose tools; do not wait for a wrapper.

## Evidence and report

Every confirmed finding needs at least one artifact that another reviewer can
open and check:

- source file/line plus the relevant state transition,
- analyzer output with the path you manually validated,
- `cast` call/storage output,
- fork-test or fuzz output with the harness source,
- minimized calldata and a trace,
- a balance/accounting delta measured on the fork.

Unsupported ideas belong in `hypotheses`, not `findings`. Decompiled Solidity,
selector guesses, and 4byte matches are never confirmed source.

Write `reports/final_report.json`. Keep the schema stable enough for downstream
readers, and add fields when they help:

```json
{
  "schema_version": 1,
  "run_id": "<RUN_ID>",
  "target": {
    "chain": "<chain alias or id>",
    "address": "0x...",
    "name": "<contract name if known>",
    "verified": true
  },
  "status": "completed | incomplete | blocked",
  "summary": "Short, evidence-based summary.",
  "findings": [
    {
      "id": "F-1",
      "title": "...",
      "severity": "critical | high | medium | low | informational",
      "confidence": "confirmed | likely",
      "contract": "0x...",
      "function": "...",
      "description": "...",
      "impact": "...",
      "evidence": ["artifacts/...", "sources/..."],
      "reproduction": "Exact command or test path.",
      "recommendation": "..."
    }
  ],
  "hypotheses": [
    {
      "title": "...",
      "reason": "...",
      "evidence": ["artifacts/..."]
    }
  ],
  "limitations": ["..."],
  "coverage": {
    "source_review": true,
    "static_analysis": false,
    "fork_testing": true,
    "fuzzing": false
  }
}
```

If you also write `reports/audit.md`, make it a readable version of the same
conclusions, not a second, inconsistent report.

## Chain reference

`references/chains.md` lists the chain aliases and chain IDs this deployment
supports. The prompt normally names the chain; if it does not, inspect the
target's deployment/transactions and record the ambiguity in the report. There
is no interactive caller in a one-shot run. Never silently audit the wrong
network.
