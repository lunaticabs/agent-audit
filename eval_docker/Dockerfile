ARG RUST_VERSION=1.88.0
ARG TOOLBOX_IMAGE=ghcr.io/trailofbits/eth-security-toolbox/ci:nightly-20260406@sha256:d99940538986a8ade09fe82d4dfd2d85ca98e7be9c705704954df482a98e4d38

FROM docker.io/library/ubuntu:22.04 AS builder

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

FROM ${TOOLBOX_IMAGE} AS runtime

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV CODEX_HOME=/root/.codex
ENV CODEX_RUNNER_DIR=/opt/agent-audit/codex-runner
ENV AGENT_AUDIT_EVAL_ENTRYPOINT=/opt/agent-audit/eval/start.sh
# ENV UV_CACHE_DIR=/tmp/uv-cache
# ENV PATH=/root/.crytic/bin:/root/.vyper/bin:/root/.foundry/bin:${PATH}
ARG NODE_MAJOR=22

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

ENV PATH=${CODEX_RUNNER_DIR}/node_modules/.bin:${PATH}

WORKDIR /opt/agent-audit

RUN mkdir -p /opt/agent-audit/eval /opt/agent-audit/bin
COPY --from=builder /build/target/release/agent-audit /opt/agent-audit/bin/agent-audit-real
COPY eval_docker/agent-audit-guard.sh /usr/local/bin/agent-audit
COPY eval_docker/AGENTS.md ./AGENTS.md
COPY eval_docker/.codex ./.codex
COPY eval_docker/start.sh /opt/agent-audit/eval/start.sh
COPY eval_docker/codex-runner/agent-audit-run.mjs ${CODEX_RUNNER_DIR}/agent-audit-run.mjs

RUN chmod +x ${CODEX_RUNNER_DIR}/agent-audit-run.mjs \
    && chmod +x /usr/local/bin/agent-audit /opt/agent-audit/bin/agent-audit-real \
    && chmod +x /opt/agent-audit/eval/start.sh \
    && mkdir -p /root/.codex /home/agent /home/logs \
    && cp /opt/agent-audit/.codex/config.toml /root/.codex/config.toml

ENTRYPOINT ["/opt/agent-audit/eval/start.sh"]

FROM runtime AS smoke-test

RUN bash -lc 'set -euo pipefail \
    && command -v bash \
    && command -v node \
    && command -v codex \
    && command -v agent-audit \
    && test -x /opt/agent-audit/bin/agent-audit-real \
    && command -v slither \
    && command -v solc-select \
    && command -v forge \
    && command -v cast \
    && command -v anvil \
    && command -v echidna \
    && test -f /opt/agent-audit/codex-runner/agent-audit-run.mjs \
    && test -x /opt/agent-audit/eval/start.sh \
    && test -d /opt/agent-audit/codex-runner/node_modules/@openai/codex-sdk \
    && node /opt/agent-audit/codex-runner/agent-audit-run.mjs --help >/dev/null \
    && /opt/agent-audit/eval/start.sh --help >/dev/null \
    && test -f /opt/agent-audit/AGENTS.md \
    && test -f /opt/agent-audit/.codex/config.toml \
    && test -f /opt/agent-audit/.codex/skills/workspace/SKILL.md \
    && test -f /opt/agent-audit/.codex/skills/slither/SKILL.md \
    && agent-audit --help >/dev/null \
    && ! agent-audit fetch-source --run-id smoke >/tmp/agent-audit-guard.out 2>&1 \
    && grep -q "disabled in EVMbench Detect" /tmp/agent-audit-guard.out \
    && grep -q "^model_provider = \"apiapi\"$" /opt/agent-audit/.codex/config.toml \
    && grep -q "^approval_policy = \"never\"$" /opt/agent-audit/.codex/config.toml'

FROM runtime AS final
