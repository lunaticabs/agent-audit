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

FROM --platform=${IMAGE_PLATFORM} ${TOOLBOX_IMAGE} AS runtime

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV CODEX_HOME=/root/.codex
ENV CODEX_RUNNER_DIR=/opt/agent-audit/codex-runner
# ENV UV_CACHE_DIR=/tmp/uv-cache
# ENV PATH=/root/.crytic/bin:/root/.vyper/bin:/root/.foundry/bin:${PATH}
ARG NODE_MAJOR=22
ARG TARGETARCH
ARG HEIMDALL_VERSION=0.9.3
ARG HEIMDALL_LINUX_AMD64_SHA256=8d58c97f56d584d5d0737d0d0895fc8c68b7b8b2b680f2a7842245789f0b4868
ARG HEIMDALL_LINUX_ARM64_SHA256=ef1d236e5e194e1722e12552b66303d3172e0c4cc267b096f7bc8a053f168df8

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        gnupg \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    case "${TARGETARCH:-amd64}" in \
        amd64) heimdall_asset="heimdall-linux-amd64"; heimdall_sha="${HEIMDALL_LINUX_AMD64_SHA256}" ;; \
        arm64) heimdall_asset="heimdall-linux-arm64"; heimdall_sha="${HEIMDALL_LINUX_ARM64_SHA256}" ;; \
        *) echo "unsupported Heimdall target architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL \
        "https://github.com/Jon-Becker/heimdall-rs/releases/download/${HEIMDALL_VERSION}/${heimdall_asset}" \
        -o /usr/local/bin/heimdall; \
    echo "${heimdall_sha}  /usr/local/bin/heimdall" | sha256sum -c -; \
    chmod +x /usr/local/bin/heimdall

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

ENV PATH=${CODEX_RUNNER_DIR}/node_modules/.bin:${PATH}

WORKDIR /opt/agent-audit

COPY --from=builder /build/target/release/agent-audit /usr/local/bin/agent-audit
COPY docker/AGENTS.md ./AGENTS.md
COPY docker/.codex ./.codex
COPY docker/codex-runner/agent-audit-run.mjs ${CODEX_RUNNER_DIR}/agent-audit-run.mjs

RUN chmod +x ${CODEX_RUNNER_DIR}/agent-audit-run.mjs \
    && mkdir -p /root/.codex /opt/agent-audit/runs \
    && cp /opt/agent-audit/.codex/config.toml /root/.codex/config.toml

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
    && test -f /opt/agent-audit/codex-runner/agent-audit-run.mjs \
    && test -d /opt/agent-audit/codex-runner/node_modules/@openai/codex-sdk \
    && node /opt/agent-audit/codex-runner/agent-audit-run.mjs --help >/dev/null \
    && test -f /opt/agent-audit/AGENTS.md \
    && test -f /opt/agent-audit/.codex/config.toml \
    && test -f /opt/agent-audit/.codex/skills/workspace/SKILL.md \
    && test -f /opt/agent-audit/.codex/skills/closed-source-evm/SKILL.md \
    && grep -q "^model_provider = \"apiapi\"$" /opt/agent-audit/.codex/config.toml \
    && grep -q "^approval_policy = \"never\"$" /opt/agent-audit/.codex/config.toml'

FROM runtime AS final
