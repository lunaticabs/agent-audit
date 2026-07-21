# EVMbench Integration

This directory contains the `agent-audit-codex` adapter and an overlay script
for a frontier-evals / EVMbench checkout.

## Overlay Into EVMbench

From this repository root:

```bash
./EVMbench/overlay.sh --evmbench-dir /path/to/evmbench
```

The script copies:

- `EVMbench/agents/agent-audit-codex/` into
  `/path/to/evmbench/evmbench/agents/agent-audit-codex/`
- the eval runtime into `agent-audit-overlay/` next to the patched Dockerfile

By default it also appends a marked `agent-audit-codex overlay` block to the
EVMbench Dockerfile, preferring `/path/to/evmbench/base/Dockerfile` when it
exists. A backup is written next to it as
`Dockerfile.agent-audit-overlay.bak`.

If the Dockerfile is in a different location:

```bash
./EVMbench/overlay.sh \
  --evmbench-dir /path/to/evmbench \
  --dockerfile path/relative/to/evmbench/Dockerfile
```

To only copy files and patch manually:

```bash
./EVMbench/overlay.sh --evmbench-dir /path/to/evmbench --no-patch-dockerfile
```

Then append:

```text
agent-audit-overlay/Dockerfile.fragment
```

to the Dockerfile used by EVMbench to build audit images. The
`agent-audit-overlay/` directory must be inside that Docker build context,
usually next to the Dockerfile.

## Run Detect

After overlaying, build EVMbench images from the EVMbench checkout:

```bash
cd /path/to/evmbench
uv sync
docker build -f ploit/Dockerfile -t ploit-builder:latest --target ploit-builder .
uv run docker_build.py --split debug
```

Run a debug Detect task:

```bash
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

For a full Detect run, replace `evmbench.audit_split=debug` with the target
split, for example `evmbench.audit_split=detect-tasks`.
