ARG RUST_VERSION=1.88.0
ARG IMAGE_PLATFORM=linux/amd64
ARG TOOLBOX_IMAGE=ghcr.io/trailofbits/eth-security-toolbox/ci:nightly-20260406@sha256:d99940538986a8ade09fe82d4dfd2d85ca98e7be9c705704954df482a98e4d38

FROM --platform=${IMAGE_PLATFORM} docker.io/library/ubuntu:22.04 AS builder

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ARG RUST_VERSION

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        pkg-config \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}"

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY dispatcher ./dispatcher
COPY src ./src
COPY xtask ./xtask

RUN cargo build --release --bin agent-audit

FROM --platform=${IMAGE_PLATFORM} docker.io/library/ubuntu:22.04 AS heimdall-builder

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ARG HEIMDALL_REV=13ae7cb02959feca08bcaafb10c9d66a28037f77
ARG HEIMDALL_RUST_VERSION=1.91.0
ARG HEIMDALL_CARGO_BUILD_JOBS=1

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        git \
        libssl-dev \
        pkg-config \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain "${HEIMDALL_RUST_VERSION}"

RUN CARGO_BUILD_JOBS="${HEIMDALL_CARGO_BUILD_JOBS}" cargo install \
    --git https://github.com/Jon-Becker/heimdall-rs \
    --rev "${HEIMDALL_REV}" \
    --locked \
    heimdall-cli

FROM --platform=${IMAGE_PLATFORM} ${TOOLBOX_IMAGE} AS runtime

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV CODEX_HOME=/root/.codex
ENV CODEX_RUNNER_DIR=/opt/agent-audit/codex-runner
# ENV UV_CACHE_DIR=/tmp/uv-cache
# ENV PATH=/root/.crytic/bin:/root/.vyper/bin:/root/.foundry/bin:${PATH}
ARG NODE_MAJOR=22
ARG FORGE_STD_REV=77041d2ce690e692d6e03cc812b57d1ddaa4d505

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        gnupg \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key \
        | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg \
    && echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_${NODE_MAJOR}.x nodistro main" \
        > /etc/apt/sources.list.d/nodesource.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends nodejs \
    && apt-get purge -y --auto-remove gnupg \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p "${CODEX_RUNNER_DIR}" \
    && cd "${CODEX_RUNNER_DIR}" \
    && npm init -y \
    && npm install --omit=dev @openai/codex @openai/codex-sdk \
    && npm cache clean --force

RUN mkdir -p /opt/agent-audit/vendor/forge-std \
    && cd /opt/agent-audit/vendor/forge-std \
    && git init \
    && git remote add origin https://github.com/foundry-rs/forge-std.git \
    && git fetch --depth 1 origin "${FORGE_STD_REV}" \
    && git checkout --detach FETCH_HEAD \
    && git submodule update --init --depth 1 \
    && rm -rf .git

ENV PATH=${CODEX_RUNNER_DIR}/node_modules/.bin:${PATH}

WORKDIR /opt/agent-audit

COPY --from=builder /build/target/release/agent-audit /usr/local/bin/agent-audit
COPY --from=heimdall-builder /root/.cargo/bin/heimdall /usr/local/bin/heimdall
COPY docker/AGENTS.md ./AGENTS.md
COPY docker/.codex/config.toml ./codex-config.toml
COPY docker/.codex/models.json ./codex-models.json
COPY docker/.codex/skills ./.codex/skills
COPY docker/codex-runner/agent-audit-run.mjs ${CODEX_RUNNER_DIR}/agent-audit-run.mjs

RUN chmod +x ${CODEX_RUNNER_DIR}/agent-audit-run.mjs \
    && mkdir -p /root/.codex /opt/agent-audit/runs \
    && cp /opt/agent-audit/codex-config.toml /root/.codex/config.toml \
    && cp /opt/agent-audit/codex-models.json /root/.codex/models.json

ENTRYPOINT ["node", "/opt/agent-audit/codex-runner/agent-audit-run.mjs"]

FROM runtime AS smoke-test

RUN bash -lc 'set -euo pipefail \
    && command -v bash \
    && command -v node \
    && command -v codex \
    && command -v agent-audit \
    && command -v slither \
    && command -v solc-select \
    && command -v forge \
    && command -v cast \
    && command -v anvil \
    && command -v echidna \
    && command -v heimdall \
    && heimdall --help >/dev/null \
    && test -f /opt/agent-audit/vendor/forge-std/src/Test.sol \
    && test -f /opt/agent-audit/codex-runner/agent-audit-run.mjs \
    && test -d /opt/agent-audit/codex-runner/node_modules/@openai/codex-sdk \
    && node /opt/agent-audit/codex-runner/agent-audit-run.mjs --help >/dev/null \
    && test -f /opt/agent-audit/AGENTS.md \
    && test ! -f /opt/agent-audit/.codex/config.toml \
    && test -f /opt/agent-audit/codex-config.toml \
    && test -f /opt/agent-audit/codex-models.json \
    && test -f /root/.codex/config.toml \
    && test -f /root/.codex/models.json \
    && test -f /opt/agent-audit/.codex/skills/workspace/SKILL.md \
    && test -f /opt/agent-audit/.codex/skills/closed-source-evm/SKILL.md \
    && grep -q "^model_provider = \"openrouter\"$" /root/.codex/config.toml \
    && grep -q "^model = \"deepseek/deepseek-v4-flash-0731\"$" /root/.codex/config.toml \
    && grep -q "^model_reasoning_effort = \"high\"$" /root/.codex/config.toml \
    && grep -q "^model_catalog_json = \"/root/.codex/models.json\"$" /root/.codex/config.toml \
    && grep -q "^model_context_window = 1048576$" /root/.codex/config.toml \
    && grep -q "^base_url = \"https://openrouter.ai/api/v1\"$" /root/.codex/config.toml \
    && grep -q "^env_key = \"OPENROUTER_API_KEY\"$" /root/.codex/config.toml \
    && grep -q "\"slug\": \"deepseek/deepseek-v4-flash-0731\"" /root/.codex/models.json \
    && grep -q "\"context_window\": 1048576" /root/.codex/models.json \
    && grep -q "\"supports_search_tool\": false" /root/.codex/models.json \
    && grep -q "^approval_policy = \"never\"$" /root/.codex/config.toml'

FROM runtime AS final
