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

The Detect path deliberately does not call the production address/chain audit
pipeline. EVMbench supplies source code in the audit directory, and the official
grader consumes only `submission/audit.md`.
