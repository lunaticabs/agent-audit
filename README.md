# agent-audit

[中文版](ch-README.md)

`agent-audit` is an agent-first smart contract audit system. It splits an EVM contract audit into four layers:

1. The Rust harness CLI under `src/` creates runs, fetches source code, prepares tool workspaces, aggregates audit materials, and archives run data.
2. The containerized audit environment under `docker/` packages the CLI, Codex SDK, Codex CLI, and common contract security tools into a single-audit runner.
3. The container-level parallel deployment system under `k3s/` uses a Redis queue and Kubernetes Jobs to run multiple runners in parallel.
4. The evaluation system under `eval_docker/` and `EVMbench/` reuses the audit environment, but adapts it for EVMbench Detect tasks against local benchmark repositories.

This root README explains the overall architecture and workflows. Detailed build, deployment, and runtime notes live in each subdirectory README.

## System Structure

| Directory | Role | Main outputs |
| --- | --- | --- |
| `src/` | Local harness CLI | `agent-audit` binary, `runs/<run_id>/` workspaces, JSON step output |
| `docker/` | Production audit runner image | `agent-audit:<tag>`, one container run per complete prompt |
| `k3s/` | Batch parallel execution layer | Redis Stream, dispatcher, one Kubernetes Job per task |
| `eval_docker/` | EVMbench-specific evaluation image | `agent-audit-eval:<tag>`, writes `submission/audit.md` |
| `EVMbench/` | EVMbench overlay | `agent-audit-codex` adapter and Dockerfile patch script |
| `dispatcher/` | k3s dispatcher program | `agent-audit-dispatcher` image, converts Redis messages into Jobs |

Overall data flow:

```text
Production single audit:
  prompt -> docker runner -> Codex agent -> agent-audit CLI -> runs/<run_id>/ -> sync-run -> Mongo

Production batch audit:
  address list -> Redis Stream -> dispatcher -> Kubernetes Jobs -> docker runner -> Mongo

EVMbench Detect:
  EVMbench audit repo -> eval runner -> Codex agent + local tools -> submission/audit.md
```

## 1. `src/`: Harness CLI

`src/` is the data preparation and archival core of the audit system. It does not make audit decisions for the agent. Instead, it prepares repeatable materials and records each step as structured JSON.

CLI entrypoint:

```bash
agent-audit <subcommand>
```

For local development:

```bash
cargo run --bin agent-audit -- <subcommand>
```

Current commands:

```text
agent-audit init-run --chain <chain> --address <address>
agent-audit fetch-source --run-id <run_id>
agent-audit run-dependency --run-id <run_id>
agent-audit prepare-slither --run-id <run_id>
agent-audit prepare-tooling --run-id <run_id>
agent-audit aggregate-materials --run-id <run_id>
agent-audit sync-run --run-id <run_id>
```

Core responsibilities:

- Create the `runs/<run_id>/` workspace plus `input/request.json` and `input/run_meta.json`.
- Fetch target contract source code through settings such as `AGENT_AUDIT_SOURCE_API_BASE`.
- Run dependency discovery and dependency analysis, writing `artifacts/dependency_*.json`.
- Prepare Slither, Foundry, and Echidna tool workspaces and build manifests.
- Aggregate audit materials into `reports/materials_manifest.json`.
- Sync run metadata and files to MongoDB through `sync-run`.

Common configuration comes from the repository-local `.env`:

```text
AGENT_AUDIT_DEFAULT_CHAIN
AGENT_AUDIT_SOURCE_API_BASE
AGENT_AUDIT_SOURCE_API_KEY
AGENT_AUDIT_SOURCE_HEADERS_JSON
AGENT_AUDIT_RPC_URL
AGENT_AUDIT_MONGO_URI
AGENT_AUDIT_MONGO_DB
AGENT_AUDIT_RUNS_DIR
```

`init-run` executes the full preparation flow: source fetch, dependency analysis, tool workspace preparation, and material aggregation. The other subcommands are used to rerun or supplement individual steps for an existing run.

## 2. `docker/`: Containerized Audit Environment

`docker/` packages the production audit environment into a runner image. The image contains:

- `agent-audit` CLI
- Node.js, `@openai/codex-sdk`, `codex`
- Slither, `solc-select`
- Foundry: `forge`, `cast`, `anvil`
- Echidna
- Container-specific `.codex` configuration and audit skills

Build:

```bash
./docker/build.sh
```

Run a single audit:

```bash
./docker/run.sh --prompt "Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth."
```

Direct Docker invocation:

```bash
docker run --rm \
  -v "$(pwd)/.env:/opt/agent-audit/.env:ro" \
  -e FULL_PROMPT="Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth." \
  agent-audit:0.1
```

The production runner accepts exactly one complete prompt:

- `FULL_PROMPT` environment variable
- or `--prompt "..."` argument

The container does not assemble business fields such as `address`, `chain`, or `instructions`. The caller must include the target, chain, audit requirements, and output requirements in the prompt. After startup, the Codex agent reads `AGENTS.md`, calls the `agent-audit` CLI and security tools as needed, then completes the audit and archival flow.

See [`docker/README.md`](docker/README.md) for details.

## 3. `k3s/`: Container-Level Parallel Deployment

`k3s/` is the single-server Kubernetes deployment for parallelizing multiple single-audit runners. It does not change the audit logic. It only turns queued tasks into one-shot Jobs.

Topology:

```text
Redis Stream
  -> agent-audit-dispatcher Deployment
  -> one Kubernetes Job per task
  -> agent-audit runner container
  -> sync-run / Mongo archive
```

Key components:

- `k3s/redis.yaml`: input queue.
- `dispatcher/`: Redis Stream to Kubernetes Job bridge.
- `k3s/dispatcher-*.yaml`: dispatcher Deployment, RBAC, configuration, and Secret.
- `k3s/runner-configmap.yaml`: runner Job template, including image, resources, TTL, and `runs/` volume.
- `k3s/runner-secret.yaml`: runner API key, source, RPC, and Mongo configuration.

Deployment entrypoint:

```bash
k3s kubectl apply -f k3s/namespace.yaml
k3s kubectl apply -f k3s/runner-secret.yaml
k3s kubectl apply -f k3s/dispatcher-secret.yaml
k3s kubectl apply -k k3s/
```

Submit a single task:

```bash
k3s kubectl -n agent-audit exec deploy/agent-audit-redis -- \
  redis-cli XADD agent-audit:tasks '*' \
    task_id audit-20260505-001 \
    full_prompt 'Check AGENTS.md and audit 0x0000000000000000000000000000000000000000 on eth.' \
    image ghcr.io/lunaticabs/agent-audit:main
```

Submit a batch from an address list:

```bash
k3s kubectl -n agent-audit port-forward svc/agent-audit-redis 6380:6379
python3 scripts/enqueue_redis.py \
  --chain eth \
  --address-file scripts/addresses/addrs.txt \
  --host 127.0.0.1 \
  --port 6380 \
  --image ghcr.io/lunaticabs/agent-audit:main
```

Use Kubernetes Job and Pod state as the runtime source of truth:

```bash
k3s kubectl -n agent-audit get jobs,pods -w
```

Redis is the input queue, not final state storage. Audit evidence and run files are written to MongoDB through the runner's internal `sync-run` flow.

See [`k3s/README.md`](k3s/README.md) for details.

## 4. `eval_docker/` and `EVMbench/`: Evaluation System

The evaluation system targets EVMbench Detect. It reuses the Codex and security tooling from the production image, but switches the data source from "chain address plus source service" to "local audit repository mounted by EVMbench".

`eval_docker/` provides the evaluation runner:

```bash
./eval_docker/build.sh
APIAPI_API_KEY=... ./eval_docker/run.sh --audit-dir /path/to/audit
```

Evaluation runner rules:

- The Codex working directory is the benchmark-provided audit repo.
- Final output must be written to `submission/audit.md`.
- The production address/chain data plane is intentionally disabled.
- Production pipeline commands such as `agent-audit init-run`, `fetch-source`, and `sync-run` are blocked by the evaluation guard.
- Temporary notes, logs, and tool outputs may be created under the benchmark-local repository, but the grader reads only `submission/audit.md`.

`EVMbench/` provides an overlay script that injects the `agent-audit-codex` adapter into an EVMbench checkout:

```bash
./EVMbench/overlay.sh --evmbench-dir /path/to/evmbench
```

Then build and run Detect from the EVMbench checkout:

```bash
cd /path/to/evmbench
uv sync
docker build -f ploit/Dockerfile -t ploit-builder:latest --target ploit-builder .
uv run docker_build.py --split debug

APIAPI_API_KEY=... \
uv run python -m evmbench.nano.entrypoint \
  evmbench.audit_split=debug \
  evmbench.mode=detect \
  evmbench.apply_gold_solution=False \
  evmbench.log_to_run_dir=True \
  evmbench.solver=evmbench.nano.solver.EVMbenchSolver \
  evmbench.solver.agent_id=agent-audit-codex \
  runner.concurrency=1
```

See [`eval_docker/README.md`](eval_docker/README.md) and [`EVMbench/README.md`](EVMbench/README.md) for details.

## Workflows

### A. Local Development and Material Preparation

```text
cargo build
  -> agent-audit init-run
  -> runs/<run_id>/input
  -> runs/<run_id>/artifacts
  -> runs/<run_id>/slither_project / foundry_project / echidna_project
  -> runs/<run_id>/reports/materials_manifest.json
```

This flow validates the CLI, source provider, dependency analysis, and material aggregation. The agent can then continue from the prepared run workspace and call Slither, Foundry, Echidna, cast/anvil, and other tools as needed.

### B. Single Production Audit

```text
docker runner
  -> Codex reads FULL_PROMPT and AGENTS.md
  -> Codex calls agent-audit CLI to prepare evidence
  -> Codex runs security tools as needed
  -> Codex writes report / findings
  -> agent-audit sync-run archives run files to Mongo
```

This flow is suitable for manual audits, prompt debugging, image validation, and reproducing a single target.

### C. Batch Parallel Audit

```text
address file or external scheduler
  -> Redis Stream message(task_id, full_prompt, image)
  -> dispatcher
  -> Kubernetes Job
  -> same docker runner as single audit
  -> Mongo archive
```

This flow is suitable for large address batches. Effective parallelism is determined mainly by k3s scheduling capacity, runner Job CPU and memory requests/limits, and node resources.

### D. EVMbench Detect Evaluation

```text
EVMbench task
  -> mounted local audit repository
  -> eval runner
  -> Codex uses local tools only
  -> submission/audit.md
```

This flow is for benchmark evaluation. It does not access the production source/RPC/Mongo data plane and does not use the address/chain pipeline to generate audit materials.

## Development Checks

Recommended command after code changes:

```bash
cargo xtask check
```

Equivalent command:

```bash
cargo run -p xtask -- check
```

Fixed check order:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

For documentation or deployment changes, at least confirm that the README and script commands still match the subdirectory documentation.
