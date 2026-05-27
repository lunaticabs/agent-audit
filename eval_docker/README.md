# EVMbench Eval Docker

This directory is the EVMbench-specific runtime image for `agent-audit-codex`.
It is based on the production Docker packaging, but its entrypoint is the
Detect benchmark adapter:

```text
/opt/agent-audit/eval/start.sh
```

Build:

```bash
./eval_docker/build.sh
```

By default this produces `agent-audit-eval:0.1`.

Direct Docker invocation should use the repository root as the build context:

```bash
docker build -f eval_docker/Dockerfile --target smoke-test -t agent-audit-eval:smoke-test .
docker build -f eval_docker/Dockerfile -t agent-audit-eval:0.1 .
```

Run against a local EVMbench audit repository:

```bash
APIAPI_API_KEY=... \
./eval_docker/run.sh --audit-dir /path/to/audit
```

The report is written to `eval_docker/submission/audit.md` unless
`--submission-dir` is provided.

Runtime contents:

- `node`
- `@openai/codex-sdk`
- `codex`
- `agent-audit` guard wrapper
- `/opt/agent-audit/bin/agent-audit-real` retained for image debugging only
- `slither`
- `solc-select`
- `forge` / `cast` / `anvil`
- `echidna`

EVMbench integration:

- `EVMbench/agents/agent-audit-codex/start.sh` is a thin wrapper that execs
  `/opt/agent-audit/eval/start.sh`.
- The eval image runs Codex in the benchmark audit directory, not in
  `/opt/agent-audit`.
- The final Detect output is always `submission/audit.md`.
- The production address/chain data pipeline is intentionally blocked for
  Detect, because EVMbench provides a local audit repository rather than a
  chain/address target.
- Local intermediate files under `runs/`, `artifacts/`, `logs/`, or `LOGS_DIR`
  are allowed when they are derived from benchmark-local source and local tool
  output. The grader still consumes only `submission/audit.md`.

The image bundles an EVMbench-specific Codex config, skills, guard wrapper, and
smart contract tooling. Skills instruct the agent to use tools directly against
the benchmark repository instead of calling production fetch, prepare, aggregate,
or sync commands.
