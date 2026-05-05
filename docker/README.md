# Docker Run

Build:

```bash
./docker/build.sh
```

By default this produces `agent-audit:0.1` and first builds a `smoke-test` target that fails early if required tools or packaged files are missing.

Direct Docker invocation should use the repository root as the build context:

```bash
docker build -f docker/Dockerfile --target smoke-test -t agent-audit:smoke-test .
docker build -f docker/Dockerfile -t agent-audit:0.1 .
```

Run:

```bash
./docker/run.sh --address 0x0000000000000000000000000000000000000000 --chain eth
```

`docker/run.sh` expects the repository-local `.env` to exist and mounts it read-only as `/opt/agent-audit/.env`. Put both `APIAPI_API_KEY` and the required `AGENT_AUDIT_*` settings there so the packaged Codex SDK entrypoint and the bundled CLI share one configuration source.

Runtime contents:

- `node`
- `@openai/codex-sdk`
- `codex`
- `agent-audit`
- `slither`
- `solc-select`
- `forge` / `cast` / `anvil`
- `echidna`

Notes:

- The runtime base image is `ghcr.io/trailofbits/eth-security-toolbox/ci:nightly-20260406`, pinned by digest in [docker/Dockerfile](/Users/lunaticabs/code/agent-audit/docker/Dockerfile).
- `agent-audit` is built in an Ubuntu 22.04 builder stage so the resulting binary is ABI-compatible with the Ubuntu 22.04 toolbox runtime.
- The image injects a dedicated container Codex bundle from `docker/.codex/`. This includes a container-specific `config.toml` plus rewritten audit skills that use direct `agent-audit` and tool binaries instead of host-development workflows such as `cargo run` or manual `.env` sourcing.
- The Docker entrypoint is a small Node program backed by the official TypeScript Codex SDK. The SDK still drives a local `codex` binary under the hood, so both `@openai/codex-sdk` and `@openai/codex` are installed in the image.
- `flake.nix` and `flake.lock` are not copied into the runtime image.
- No batch scheduler is included; the entrypoint runs exactly one Codex audit task.
- The Docker build context root is the repository root. Runtime-specific files stay under `docker/`; repository files are copied directly by the Dockerfile.
- `docker/build.sh` first builds the `smoke-test` target, then builds the final runtime image. Override the default name with `IMAGE_REPO=...` and `IMAGE_TAG=...`.
- `solc` remains run-dependent. The toolbox image includes `solc-select`, but the exact compiler version needed is only known after `fetch-source` discovers the target contract settings. The runtime therefore still needs network access if a run must install a missing `solc` version on demand.
- The container now emits human-readable text logs. Major lifecycle steps, SDK event summaries, and the final assistant output are printed in a terminal-friendly format instead of raw JSON.

Run a single audit directly with `docker run`:

```bash
docker run --rm \
  -v "$(pwd)/.env:/opt/agent-audit/.env:ro" \
  agent-audit:0.1 \
  --address 0x0000000000000000000000000000000000000000 \
  --chain eth
```

Append extra instructions with `--instructions "..."`.
