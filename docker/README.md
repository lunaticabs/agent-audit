# Single-audit Docker runner

`docker/` packages the complete audit environment into one image. One container
run performs exactly one autonomous audit with DeepSeek Harness in headless
mode.

## Build

```bash
./docker/build.sh
```

The default tags are `agent-audit:2.0.0`; override with `IMAGE_REPO` and
`IMAGE_TAG`. Build targets:

- `runtime` — the default image.
- `smoke-test` — packaging checks for DSH, the security tools, and the audit
  skill files.
- `final` — same as runtime; used by CI.

The build context is the repository root:

```bash
docker build -f docker/Dockerfile -t agent-audit:2.0.0 .
docker build -f docker/Dockerfile --target smoke-test -t agent-audit:smoke-test .
```

GitHub Actions publishes the runtime and dispatcher images to GHCR. It runs the
`smoke-test` target on pull requests before publishing.

## Configure

Create a repository-local `.env` from `.env.example` (or pass the same variables in the environment):

```text
DEEPSEEK_API_KEY=...
DSH_MODEL=deepseek-flash
DSH_REASONING_EFFORT=max
ETHERSCAN_API_KEY=...
ETHERSCAN_API_URL=https://api.etherscan.io/v2/api
RPC_URL=...

# Optional: per-run MongoDB mirror. Persistence is skipped when URI is unset.
AGENT_AUDIT_MONGO_URI=mongodb://user:password@host:27017/?authSource=admin
AGENT_AUDIT_MONGO_DB=agent
```

`DEEPSEEK_API_KEY` is required. The image uses the official DeepSeek endpoint
through DSH's `deepseek-official` provider; OpenRouter is not used.

`DSH_MODEL` defaults to `deepseek-flash`. For a stronger audit model, set
`deepseek-v4-pro`. `DSH_REASONING_EFFORT` accepts `off`, `low`, `high`, or
`max`; the image default is `max`.

## Environment variables

For a manual single audit, put values in the repository `.env`; it is
gitignored. `docker/run.sh` mounts it read-only at `/opt/agent-audit/.env` and
the entrypoint loads it. The script also forwards these exported shell
variables into the container if present:

```text
DEEPSEEK_API_KEY DSH_MODEL DSH_REASONING_EFFORT
ETHERSCAN_API_KEY ETHERSCAN_API_URL RPC_URL
TASK_ID RUN_ID AGENT_AUDIT_PERSIST_DIR
AGENT_AUDIT_MONGO_URI AGENT_AUDIT_MONGO_DB
AGENT_AUDIT_MONGO_RUNS_META_COLLECTION AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION
AGENT_AUDIT_MONGO_MAX_INLINE_FILE_BYTES AGENT_AUDIT_MONGO_TIMEOUT_MS
AGENT_AUDIT_MONGO_RETRIES AGENT_AUDIT_MONGO_PERSIST
AGENT_AUDIT_RPC_TIMEOUT_MS
```

An already-exported variable wins over the same key in `.env`; image defaults
apply last. `DEEPSEEK_API_KEY` is required. `ETHERSCAN_API_KEY` and `RPC_URL`
are strongly recommended; source/bytecode work and fork tests need them.

For a direct `docker run`, either mount `.env` the same way (recommended) or
pass `--env-file .env` / `-e NAME=value`. The prompt is not an environment
variable for the agent: pass it as `--prompt` or `FULL_PROMPT`.

## Run one audit

```bash
./docker/run.sh --prompt "Audit 0x0000000000000000000000000000000000000000 on eth."
```

Or pass the prompt through the environment:

```bash
FULL_PROMPT="Audit 0x0000000000000000000000000000000000000000 on base." ./docker/run.sh
```

Direct Docker invocation:

```bash
docker run --rm \
  -v "$(pwd)/.env:/opt/agent-audit/.env:ro" \
  -v "$(pwd)/runs:/opt/agent-audit/runs" \
  -e FULL_PROMPT="Audit 0x0000000000000000000000000000000000000000 on eth." \
  agent-audit:2.0.0
```

The runner accepts one complete prompt only. It does not assemble `address`,
`chain`, or `instructions` fields. The caller includes the target, network, and
any scope in the prompt. The entrypoint passes that text to DSH on stdin as the
initial user message, and also keeps a durable copy in `input/prompt.txt`.
DSH's skill catalog triggers the `evm-audit` skill from its description, so the
prompt needs no routing or role text.

`TASK_ID` is optional; use it for an external scheduler. `RUN_ID` overrides the
generated run-directory name. Both are normalized to a safe directory name.

## Workspace and persistence

Each container starts in `/opt/agent-audit/runs/<run_id>` and creates:

```text
input/prompt.txt
input/request.json
input/run_meta.json
artifacts/
reports/
logs/agent.stdout.log
logs/agent.stderr.log
logs/stats.log
logs/persist.log
stats/metrics.json
```

The entrypoint runs DSH headless with the run directory as its working
directory. When the audit exits, an exit trap writes `input/run_meta.json`
(status, exit code, timings), captures the final answer in
`reports/final_answer.md`, derives `stats/metrics.json` (set
`AGENT_AUDIT_STATS=0` to skip it), persists the run to MongoDB when
`AGENT_AUDIT_MONGO_URI` is set (set `AGENT_AUDIT_MONGO_PERSIST=0` to skip it),
and optionally copies the run to `AGENT_AUDIT_PERSIST_DIR`.

`stats/` is derived data: severity normalization, evidence-path checks, fork
reproduction, tool usage, and anomalies. The collector never changes the audit
exit code; failures land in `logs/stats.log`. Aggregation happens outside the
container, see [../tools/audit_stats/README.md](../tools/audit_stats/README.md).

The MongoDB mirror is also one run per write: one `runs_meta` document with
top-level `target_address` / `target_chain` and embedded `stats`, plus one
`runs_files` document for every regular file under that run's directory,
including logs, stats, build output, and root files. `.run.lock`,
`logs/persist.log`, and symlinks that escape the run are skipped, and oversized
files are listed in
`skipped_files`. The meta document also carries a `materials` manifest with
`rel_path`, `kind`, `size_bytes`, and `sha256` for each file. `block_number` is
taken from saved evidence first, then fetched with `eth_blockNumber` from
`RPC_URL`; the source is stored in `block_number_source`. Batch-level
directories are never walked. A database failure lands in
`logs/persist.log` and does not change the audit exit code. See
[../tools/run_persist/README.md](../tools/run_persist/README.md).

`docker/run.sh` bind-mounts the host `runs/` directory at
`/opt/agent-audit/runs`, so evidence is durable as it is written. k3s uses a
hostPath volume at the same mount point; see [../k3s/README.md](../k3s/README.md).
The run directory remains the source of truth even when MongoDB is enabled.

## Runtime contents

- Node.js 24
- `@deepseek-ai/dsh` 0.2.1-rc.3 (`dsh` CLI, headless profile)
- Slither and `solc-select`
- Foundry: `forge`, `cast`, `anvil`
- Echidna
- Heimdall
- `jq`, `curl`, `git`, and the usual shell
- Python 3 with `pymongo` for the per-run MongoDB mirror
- vendored `forge-std` for offline harnesses
- the `evm-audit` skill under `DSH_HOME` (the run's entry point)

The base image is the pinned Trail of Bits eth-security-toolbox image. Heimdall
is built from a pinned upstream revision in an Ubuntu 22.04 builder so its
binary matches the runtime glibc. `solc` remains run-dependent: the image does
not pin every compiler, so the agent runs `solc-select` itself when source
review requires a specific version.

## DSH configuration

The image ships a home-level DSH patch at `dsh-home/cordis.patch.yml`:

- model/provider: `deepseek-official` + `DSH_MODEL` (default `deepseek-flash`)
- reasoning effort: `DSH_REASONING_EFFORT` (default `max`)
- no role or persona prompt
- full sandbox access and no approval prompts inside the container
- session-log and package-inventory request extensions disabled
- auxiliary title-generation model call disabled
- telemetry disabled by default

The single main skill is `dsh-home/skills/evm-audit/SKILL.md`. Its root is a
short router; tool commands, supported chains, and the report shape live in
`references/` and load on demand. The skill description triggers on audit
tasks. There is no role prompt, no per-tool workflow skill, and no separate
AGENTS.md in the image.

## Security boundary

The container is the isolation boundary. DSH runs with
`danger-full-access` and `approval_policy: never` so the agent can use its shell
and tools without an interactive answerer. Treat the container, the mounted
run directory, and the credentials in `.env` as one trust boundary. Do not mount
a host directory the agent should be able to modify beyond `runs/`.

The agent can reach the network (Etherscan, RPC, package installs, web search).
If you need egress restrictions, enforce them at the Docker/k3s network layer,
not inside the agent prompt.
