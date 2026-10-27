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
```

`DEEPSEEK_API_KEY` is required. The image uses the official DeepSeek endpoint
through DSH's `deepseek-official` provider; OpenRouter is not used.

`DSH_MODEL` defaults to `deepseek-flash`. For a stronger audit model, set
`deepseek-v4-pro`. `DSH_REASONING_EFFORT` accepts `off`, `low`, `high`, or
`max`; the image default is `max`.

## Run one audit

```bash
./docker/run.sh --prompt "Audit 0x0000000000000000000000000000000000000000 on eth. Start by loading the evm-audit skill."
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
any scope in the prompt.

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
```

The entrypoint runs DSH headless with the run directory as its working
directory. When the audit exits, an exit trap writes `input/run_meta.json`
(status, exit code, timings), captures the final answer in
`reports/final_answer.md`, optionally copies the run to
`AGENT_AUDIT_PERSIST_DIR`, and calls `sync`.

`docker/run.sh` bind-mounts the host `runs/` directory at
`/opt/agent-audit/runs`, so evidence is durable as it is written. k3s uses a
hostPath volume at the same mount point; see [../k3s/README.md](../k3s/README.md).
There is no MongoDB or other archive backend.

## Runtime contents

- Node.js 24
- `@deepseek-ai/dsh` 0.2.1-rc.3 (`dsh` CLI, headless profile)
- Slither and `solc-select`
- Foundry: `forge`, `cast`, `anvil`
- Echidna
- Heimdall
- `jq`, `curl`, `git`, and the usual shell
- vendored `forge-std` for offline harnesses
- the `evm-audit` skill and audit instructions under `DSH_HOME`

The base image is the pinned Trail of Bits eth-security-toolbox image. Heimdall
is built from a pinned upstream revision in an Ubuntu 22.04 builder so its
binary matches the runtime glibc. `solc` remains run-dependent: the image does
not pin every compiler, so the agent runs `solc-select` itself when source
review requires a specific version.

## DSH configuration

The image ships a home-level DSH patch at `dsh-home/cordis.patch.yml`:

- model/provider: `deepseek-official` + `DSH_MODEL` (default `deepseek-flash`)
- reasoning effort: `DSH_REASONING_EFFORT` (default `max`)
- full sandbox access and no approval prompts inside the container
- session-log and package-inventory request extensions disabled
- auxiliary title-generation model call disabled
- telemetry disabled by default

The audit instructions live at `dsh-home/AGENTS.md`; the single main skill is
`dsh-home/skills/evm-audit/SKILL.md`, with the supported-chain table in
`references/chains.md`. There are no per-tool workflow skills: tools are
declared in the main skill and used directly.

## Security boundary

The container is the isolation boundary. DSH runs with
`danger-full-access` and `approval_policy: never` so the agent can use its shell
and tools without an interactive answerer. Treat the container, the mounted
run directory, and the credentials in `.env` as one trust boundary. Do not mount
a host directory the agent should be able to modify beyond `runs/`.

The agent can reach the network (Etherscan, RPC, package installs, web search).
If you need egress restrictions, enforce them at the Docker/k3s network layer,
not inside the agent prompt.
