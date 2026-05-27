# agent-audit-codex EVMbench Adapter

This directory is an EVMbench Detect agent adapter for the frontier-evals agent
interface. Copy or overlay it into an EVMbench checkout at:

```text
project/evmbench/evmbench/agents/agent-audit-codex
```

Then run Detect tasks with agent id `agent-audit-codex`.

Build the eval runtime from the repository root:

```bash
./eval_docker/build.sh
```

For official EVMbench image builds, use the repository-level overlay helper
instead of manually editing the EVMbench Dockerfile:

```bash
./EVMbench/overlay.sh --evmbench-dir /path/to/evmbench
```

The helper writes `agent-audit-overlay/` into the Docker build context next to
the patched Dockerfile so EVMbench's `docker_build.py` can build
`evmbench/base:latest`.

The EVMbench-facing `start.sh` in this directory is a thin wrapper. It expects
the audit container to include the eval Docker runtime and execs:

```text
/opt/agent-audit/eval/start.sh
```

The benchmark-specific runner behavior lives in `eval_docker/`, not in the
production `docker/` directory.

At runtime, the eval entrypoint keeps `AGENT_AUDIT_PROJECT_ROOT=/opt/agent-audit`
for bundled Codex config and skills, but starts Codex in the benchmark audit
repository through `CODEX_WORKDIR`. It writes the grader output to the EVMbench
submission path, usually `/home/agent/submission/audit.md` or
`/home/oai/submission/audit.md`.

The Detect path deliberately blocks the production address/chain data pipeline.
EVMbench supplies source code in the audit directory, and the official grader
consumes only `submission/audit.md`. The agent may create local intermediate
materials under `runs/`, `artifacts/`, `logs/`, or `LOGS_DIR`, but those files
must be derived from benchmark-local source and local tool output.
