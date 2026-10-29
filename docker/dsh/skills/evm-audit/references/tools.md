# Tools and on-chain data

Environment: `ETHERSCAN_API_KEY`, `ETHERSCAN_API_URL` (default
`https://api.etherscan.io/v2/api`), `RPC_URL`, `RUN_ID`, `RUN_DIR`.
Installed: `curl`, `jq`, `cast`, `forge`, `anvil`, `slither`, `solc-select`,
`echidna`, `heimdall`.

## Etherscan v2

One key works across supported chains; pass `chainid`. Cache responses under
`artifacts/`. The free tier allows roughly five requests per second.

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
  | tee artifacts/etherscan_getsourcecode.json | jq .
```

`getsourcecode` returns `SourceCode`, `ABI`, `ContractName`,
`CompilerVersion`, optimization settings, `EVMVersion`, `Proxy`, and
`Implementation`. Empty `SourceCode` means unverified. A proxy can point to an
implementation with different verification status.

Other actions:

```bash
# ABI and creation bytecode
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=contract" --data-urlencode "action=getabi" \
  --data-urlencode "address=$ADDR" --data-urlencode "apikey=$ETHERSCAN_API_KEY"
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=contract" --data-urlencode "action=getbytecode" \
  --data-urlencode "address=$ADDR" --data-urlencode "apikey=$ETHERSCAN_API_KEY"

# Runtime code and storage through the proxy module
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=proxy" --data-urlencode "action=eth_getCode" \
  --data-urlencode "address=$ADDR" --data-urlencode "tag=latest" \
  --data-urlencode "apikey=$ETHERSCAN_API_KEY"
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=proxy" --data-urlencode "action=eth_getStorageAt" \
  --data-urlencode "address=$ADDR" --data-urlencode "position=0x0" \
  --data-urlencode "tag=latest" --data-urlencode "apikey=$ETHERSCAN_API_KEY"

# History
curl -sS --get "$BASE" --data-urlencode "chainid=$CID" \
  --data-urlencode "module=account" --data-urlencode "action=txlist" \
  --data-urlencode "address=$ADDR" --data-urlencode "startblock=0" \
  --data-urlencode "endblock=latest" --data-urlencode "sort=desc" \
  --data-urlencode "apikey=$ETHERSCAN_API_KEY"
```

Also useful: `contract/getcontractcreation`, `account/balance`,
`account/tokentx`, `logs/getLogs`, `block/getblocknobytime`.
If a chain is unsupported by Etherscan v2, use `RPC_URL` and the chain's official
explorer; do not substitute another network.

## RPC and state probes

```bash
cast chain-id --rpc-url "$RPC_URL"
cast code "$ADDR" --rpc-url "$RPC_URL"
cast storage "$ADDR" 0x0 --rpc-url "$RPC_URL"
cast call "$ADDR" "owner()(address)" --rpc-url "$RPC_URL"
cast balance "$ADDR" --rpc-url "$RPC_URL"
```

EIP-1967 slots:

- implementation: `0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc`
- admin: `0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103`
- beacon: `0xa3f0ad74e5423aebfd80d3ef4346578335a9a72aeaee59ff6cb3582b35133d50`

Minimal-proxy runtime prefix:
`363d3d373d3d3d363d73...5af43d82803e903d91602b57fd5bf3`.
Verify a candidate implementation has code before auditing it.

## Verified source

Save the source bundle and compiler settings under `sources/`.

```bash
solc-select install <version> && solc-select use <version>
slither . --json artifacts/slither.json
mkdir -p lib && ln -sfn /opt/agent-audit/vendor/forge-std lib/forge-std
forge test --fork-url "$RPC_URL" -vvvv
```

`forge install` needs network; the vendored `forge-std` is the offline default.
Resolve dependencies from the source, Etherscan, the project repository, or
package registries.

## Unverified source

```bash
mkdir -p artifacts/heimdall
heimdall disassemble "$ADDR" --rpc-url "$RPC_URL" --output artifacts/heimdall/disassemble
heimdall decompile "$ADDR" --rpc-url "$RPC_URL" --output artifacts/heimdall/decompile
heimdall cfg "$ADDR" --rpc-url "$RPC_URL" --output artifacts/heimdall/cfg
cast 4byte 0xa9059cbb
curl -sS "https://www.4byte.directory/api/v1/signatures/?hex_signature=0xa9059cbb" \
  | tee artifacts/4byte_a9059cbb.json | jq .
```

Check `heimdall --help` for flag changes. Treat decompiler and 4byte output as
heuristic evidence, not source.

Fork harness pattern:

```solidity
(bool ok, bytes memory ret) = target.call{value: value}(data);
```

Use `vm.createSelectFork(vm.envString("RPC_URL"))`, `vm.deal`, `vm.prank` or
`vm.startPrank`, and `bound`. Save the harness and trace under `artifacts/`.

Record the block you forked at in `artifacts/pinned_block.json`:

```json
{ "block": 21000000, "chain_id": 1 }
```

Saving the raw `eth_getBlockByNumber` response for that block also works.

After the run exits, the harness is replayed at that block and at an earlier
one; without this file the replay falls back to the chain head and its verdict
is weaker. Keep fork markers inside a function body (`setUp`, a helper, or the
test itself) — a marker in a comment is not counted as fork evidence.

Property/sequence fuzzing:

```bash
ECHIDNA_RPC_URL="$RPC_URL" echidna . --contract <HarnessContract> --config echidna.yaml
```

Test concrete economic properties (value movement, accounting, roles, limits)
and minimize any counterexample before reporting it.
