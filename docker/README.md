# Docker Run

Build:

```bash
./docker/build.sh
```

Direct Docker invocation should use the repository root as the build context:

```bash
docker build -f docker/Dockerfile -t agent-audit-codex .
```

Runtime contents:

- `codex`
- `agent-audit`
- `slither`
- `solc-select`
- `forge` / `cast` / `anvil`
- `echidna`

Notes:

- The runtime base image is `ghcr.io/trailofbits/eth-security-toolbox/ci:nightly-20260406`, pinned by digest in [docker/Dockerfile](/Users/lunaticabs/code/agent-audit/docker/Dockerfile).
- `agent-audit` is built in an Ubuntu 22.04 builder stage so the resulting binary is ABI-compatible with the Ubuntu 22.04 toolbox runtime.
- The image injects a dedicated container Codex config from `docker/config.toml`. It fixes provider, model, `wire_api = "responses"`, `sandbox_mode = "danger-full-access"`, `approval_policy = "never"`, and `shell_environment_policy.inherit = "all"`.
- `flake.nix` and `flake.lock` are not copied into the runtime image.
- No batch scheduler is included; the entrypoint runs exactly one Codex audit task.
- The Docker build context root is the repository root. Runtime-specific files stay under `docker/`; repository files are copied directly by the Dockerfile.
- `docker/build.sh` is now a thin wrapper around `docker build -f docker/Dockerfile ... .`.
- `solc` remains run-dependent. The toolbox image includes `solc-select`, but the exact compiler version needed is only known after `fetch-source` discovers the target contract settings. The runtime therefore still needs network access if a run must install a missing `solc` version on demand.

Run a single audit:

```bash
docker run --rm \
  -e APIAPI_API_KEY=... \
  -e AGENT_AUDIT_SOURCE_API_BASE=... \
  -e AGENT_AUDIT_SOURCE_API_KEY=... \
  -e AGENT_AUDIT_RPC_URL=... \
  -e AGENT_AUDIT_MONGO_URI=... \
  -e AGENT_AUDIT_MONGO_DB=agent_audit \
  -e AGENT_AUDIT_MONGO_RUNS_META_COLLECTION=runs_meta \
  -e AGENT_AUDIT_MONGO_RUNS_FILES_COLLECTION=runs_files \
  -v "$(pwd)/runs:/opt/agent-audit/runs" \
  agent-audit-codex \
  0x0000000000000000000000000000000000000000 eth
```

Optional third argument appends extra instructions to the Codex prompt.
