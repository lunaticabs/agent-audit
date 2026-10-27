# agent-audit

[中文版](ch-README.md)

`agent-audit` is an agent-first smart-contract audit system. DeepSeek Harness
runs one autonomous audit inside a container that already contains the chain
data, source data, and security tools. Two deployment modes share the same
image:

- **Single audit** — `docker/run.sh` launches one container for one prompt.
- **Parallel audit** — k3s dispatcher turns Redis messages into one-shot Jobs.

There is no database archive, no Rust audit CLI, and no fixed audit pipeline.
The run directory is the durable artifact; the agent works against Etherscan,
RPC, and the installed tools directly.

## What changed

- The agent is **DeepSeek Harness in headless CLI mode**, using the official
  DeepSeek API (`deepseek-official`), not Codex/OpenRouter.
- The old Rust `agent-audit` pipeline (`src/`) is gone. Its source fetching,
  dependency heuristics, material aggregation, and Mongo archive were replaced
  by direct Etherscan v2/RPC access and a small container entrypoint.
- `eval_docker/` and `EVMbench/` were removed. This repository is for audits,
  not agent benchmarking.
- The per-tool workflow skills were replaced by **one main `evm-audit` skill**
  plus a declared tool catalog. The agent chooses its own method.
- MongoDB archival is gone. Each run is persisted under `runs/<run_id>/`;
  Docker bind-mounts it and k3s mounts a node-local hostPath.
- The supported chain set, the container tool list, and the k3s deployment are
  preserved.

## Architecture

```text
Single audit:
  prompt -> runner container -> DSH headless -> Etherscan/RPC/tools
         -> runs/<run_id>/ -> host runs/ (bind mount)

Parallel audit:
  address list -> Redis Stream -> dispatcher -> Kubernetes Jobs
               -> same runner image -> /var/lib/agent-audit/runs/<run_id>
```

| Path | Role |
| --- | --- |
| `docker/` | Runtime image, single-audit entrypoint, DSH instructions and skill |
| `k3s/` | Redis queue, dispatcher Deployment, runner Job template, hostPath runs |
| `dispatcher/` | Rust Redis-to-Kubernetes-Job bridge |
| `scripts/` | Batch enqueue helper and address lists |
| `tools/` | Post-run statistics (`audit_stats`) |
| `xtask/` | Workspace formatting, linting, and test checks |
| `docs/` | Long-form reports |

## Quick start: one audit

Requirements: Docker and a DeepSeek API key. An Etherscan key and an archive RPC
are strongly recommended; closed-source and fork-based work needs RPC.

```bash
cp .env.example .env
# fill DEEPSEEK_API_KEY, ETHERSCAN_API_KEY, RPC_URL
./docker/build.sh
./docker/run.sh --prompt "Audit 0x0000000000000000000000000000000000000000 on eth."
```

The container accepts exactly one complete prompt, through `FULL_PROMPT` or
`--prompt`. Put the address, chain, and any scope or output requirements in the
prompt. See [docker/README.md](docker/README.md).

A finished run lands in `runs/<run_id>/`:

```text
input/prompt.txt              # durable copy of the caller request
input/request.json            # normalized request
input/run_meta.json           # status, exit code, timings
artifacts/                    # evidence produced by the agent
sources/                      # optional fetched source
reports/final_report.json     # required structured report
reports/final_answer.md       # captured DSH stdout
logs/agent.stdout.log
logs/agent.stderr.log
logs/stats.log                # stats collector output
stats/metrics.json            # derived post-run metrics
```

The entrypoint writes the final metadata and captures stdout on exit. The
agent does not run a persistence command; there is none.

## Audit capability

**Open source and verified.** The agent fetches source, ABI, compiler settings,
proxy information, and history directly from Etherscan v2. It resolves
dependencies itself from sources, repositories, and chain data. It uses
`slither`, `forge`, `cast`, `anvil`, `solc-select`, and `echidna` as needed.

**Closed source and unverified.** The agent fetches runtime bytecode, resolves
proxy implementations, disassembles/decompiles with Heimdall, recovers selectors,
and builds fork-backed Foundry or Echidna harnesses to test concrete economic
properties. Decompiler output is treated as heuristic evidence.

**Chains.** The current alias set is preserved: Ethereum and testnets, BNB,
Polygon, Base, Arbitrum, OP, Avalanche, Linea, Blast, Scroll, Mantle, Gnosis,
Celo, zkSync, opBNB, Moonbeam/Moonriver, BitTorrent, Fraxtal, Sonic, Sei, Taiko,
Unichain, and World Chain. The full table is in
[docker/dsh/skills/evm-audit/references/chains.md](docker/dsh/skills/evm-audit/references/chains.md).

**Tools.** The runtime keeps Node/DSH, Slither, `solc-select`, Foundry
(`forge`/`cast`/`anvil`), Echidna, and Heimdall. It adds `jq` for direct JSON
API work. The tool catalog and commands are declared in the main skill; there
are no separate slither/forge/anvil/echidna skills.

**Data access.** The agent gets `ETHERSCAN_API_KEY`, `ETHERSCAN_API_URL`, and
`RPC_URL` and calls them with `curl`/`cast`. The old heuristic dependency
discovery and source-provider CLI are not in the loop.

## Skill design

`docker/dsh/skills/evm-audit/SKILL.md` is the only audit skill. Its root is a
short router: workspace/output contract, a few non-obvious checks, and pointers.
Details load on demand:

- `references/tools.md` — Etherscan v2, RPC, Foundry, Slither, Echidna, Heimdall.
- `references/report.md` — report shape and evidence requirements.
- `references/chains.md` — supported chain aliases and IDs.

The skill is the run's entry point. DSH exposes it in the skill catalog and the
audit task triggers it from its description; there is no role prompt, no forced
workflow, and no separate AGENTS.md in the runtime image. The root file stays
short and routes to references for tools, chains, and report format.

## k3s deployment

```bash
cp k3s/runner-secret.example.yaml k3s/runner-secret.yaml
cp k3s/dispatcher-secret.example.yaml k3s/dispatcher-secret.yaml
# fill DEEPSEEK_API_KEY, ETHERSCAN_API_KEY, RPC_URL
sudo mkdir -p /var/lib/agent-audit/runs
k3s kubectl apply -f k3s/namespace.yaml
k3s kubectl apply -f k3s/runner-secret.yaml
k3s kubectl apply -f k3s/dispatcher-secret.yaml
k3s kubectl apply -k k3s/
```

Submit one task:

```bash
k3s kubectl -n agent-audit exec deploy/agent-audit-redis -- \
  redis-cli XADD agent-audit:tasks '*' \
    task_id audit-20261026-001 \
    full_prompt 'Audit 0x0000000000000000000000000000000000000000 on eth.'
```

Submit a batch:

```bash
python3 scripts/enqueue_redis.py \
  --chain eth \
  --address-file scripts/addresses/addrs.txt \
  --image ghcr.io/lunaticabs/agent-audit:rebuild
```

Each Job mounts a node-local `hostPath` at `/opt/agent-audit/runs`, so the run
survives Pod cleanup. See [k3s/README.md](k3s/README.md) for the full deployment,
secret, and multi-node caveats.

## Run statistics

Every finished run gets a derived `stats/metrics.json` (the entrypoint collects
it on exit; `AGENT_AUDIT_STATS=0` disables it). Aggregate a round:

```bash
# collect metrics for runs that do not have them yet
PYTHONPATH=tools python3 -m audit_stats collect --runs-root runs

# round report: summary.json, REPORT.md, runs.csv, findings.csv
PYTHONPATH=tools python3 -m audit_stats report --runs-root runs --expected 120
```

The report covers findings and severity, fork-reproduction coverage (linked test
passed, restricted to fork-backed runs), trace-based function coverage (which
ABI functions the run exercised), cross-run stability (title Jaccard and
severity drift when a target was audited more than once), evidence resolution,
declared-vs-observed coverage, tool usage, schema grades, anomalies, and latency
percentiles, with strata by model, chain, verified status, and schema grade. It treats
`final_report.json` as untrusted: severity is normalized, evidence paths must
stay inside the run directory, and a bad report lands in the funnel with an
anomaly code instead of an exception.

`stats/` is derived and safe to delete. On k3s the metrics land on the
node-local hostPath with the run; multi-node aggregation rsyncs only
`*/stats/metrics.json`. See [docs/audit-statistics.md](docs/audit-statistics.md)
for the metric definitions and k3s collection plan, and
[tools/audit_stats/README.md](tools/audit_stats/README.md) for the CLI and
schema.

## Development checks

Only the dispatcher and xtask remain Rust:

```bash
cargo xtask check
```

Equivalent commands:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Docker packaging checks:

```bash
docker build -f docker/Dockerfile --target smoke-test -t agent-audit:smoke-test .
```

## Results

The earlier pipeline scanned 8,255 Ethereum mainnet contracts end to end and
confirmed 6,466 vulnerabilities, reproducing exploits on 432 contracts against
forked mainnet state. The report is still available at
[docs/chain-scan-report.md](docs/chain-scan-report.md).

## Security notes

- The container is the isolation boundary. DSH runs with full filesystem access
  and no approval prompts inside it. In k3s, all Jobs share the node-local runs
  root, so a runner can read sibling runs; use per-run volumes for tenant
  isolation.
- `.env` holds the DeepSeek, Etherscan, and RPC credentials. Do not commit it;
  do not mount it into a container you do not trust.
- Session-log and package-inventory upload extensions are disabled in the DSH
  home patch. Telemetry is disabled by default.
- `runs/` contains audit evidence and may contain sensitive target details.
