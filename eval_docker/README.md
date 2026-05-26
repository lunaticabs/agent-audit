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
./eval_docker/run.sh --audit-dir /path/to/audit
```

The report is written to `eval_docker/submission/audit.md` unless
`--submission-dir` is provided.

Runtime contents:

- `node`
- `@openai/codex-sdk`
- `codex`
- `agent-audit`
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
- The production address/chain pipeline is intentionally not used for Detect,
  because EVMbench provides a local audit repository rather than a chain/address
  target.

The image still bundles the production Codex config, skills, CLI, and smart
contract tooling so the benchmark uses the same packaged runtime environment.
