# Docker Run

Build:

```bash
./docker/build.sh
```

Runtime contents:

- `codex`
- `agent-audit`
- `slither`
- `solc-select`
- `forge` / `cast` / `anvil`
- `echidna`

Notes:

- `flake.nix` and `flake.lock` are not copied into the runtime image.
- Python is included only because `slither-analyzer` and `solc-select` require it at runtime.
- No batch scheduler is included; the entrypoint runs exactly one Codex audit task.
- `docker/context/` is generated build input and is ignored by git.
- The generated context includes only the files needed to build `agent-audit` and ship the Codex runtime assets.
- `docker/build.sh` checks that `docker buildx` is available, then uses `docker build`, which Docker documents as a wrapper around Buildx for the default builder.
- If `docker buildx` is missing, install the Docker buildx CLI plugin before building. On Ubuntu with Docker's official apt repo, this is typically the `docker-buildx-plugin` package.

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
