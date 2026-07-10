ARG RUST_VERSION=1.88.0
ARG HEIMDALL_RUST_VERSION=1.91.0
ARG HEIMDALL_VERSION=0.9.3
ARG PROMPT_PROFILE=both
ARG IMAGE_PLATFORM=linux/amd64
ARG CARGO_BUILD_JOBS=2
ARG TOOLBOX_IMAGE=ghcr.io/trailofbits/eth-security-toolbox/ci:nightly-20260406@sha256:d99940538986a8ade09fe82d4dfd2d85ca98e7be9c705704954df482a98e4d38

FROM --platform=${IMAGE_PLATFORM} docker.io/library/ubuntu:22.04 AS builder

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ARG RUST_VERSION
ARG CARGO_BUILD_JOBS
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

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

FROM --platform=${IMAGE_PLATFORM} docker.io/library/rust:${HEIMDALL_RUST_VERSION}-bookworm AS heimdall-builder

ARG HEIMDALL_VERSION
ARG CARGO_BUILD_JOBS
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

COPY --from=builder /build/target/release/agent-audit /tmp/agent-audit-build-order

RUN rm /tmp/agent-audit-build-order \
    && cargo install --locked \
    --git https://github.com/Jon-Becker/heimdall-rs.git \
    --tag "${HEIMDALL_VERSION}" \
    heimdall-cli

FROM --platform=${IMAGE_PLATFORM} docker.io/library/ubuntu:22.04 AS prompt-builder

ARG PROMPT_PROFILE

COPY docker/osprompt /input/osprompt
COPY docker/csprompt /input/csprompt

RUN case "${PROMPT_PROFILE}" in \
        both|open_source|closed_source) ;; \
        *) echo "invalid PROMPT_PROFILE=${PROMPT_PROFILE}" >&2; exit 2 ;; \
    esac \
    && mkdir -p /output/prompts \
    && if [ "${PROMPT_PROFILE}" = "both" ] || [ "${PROMPT_PROFILE}" = "open_source" ]; then cp -a /input/osprompt /output/prompts/osprompt; fi \
    && if [ "${PROMPT_PROFILE}" = "both" ] || [ "${PROMPT_PROFILE}" = "closed_source" ]; then cp -a /input/csprompt /output/prompts/csprompt; fi

FROM --platform=${IMAGE_PLATFORM} ${TOOLBOX_IMAGE} AS runtime

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV CODEX_HOME=/root/.codex
ENV CODEX_RUNNER_DIR=/opt/agent-audit/codex-runner
# ENV UV_CACHE_DIR=/tmp/uv-cache
# ENV PATH=/root/.crytic/bin:/root/.vyper/bin:/root/.foundry/bin:${PATH}
ARG NODE_MAJOR=22
ARG PROMPT_PROFILE

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

COPY --from=builder /build/target/release/agent-audit /usr/local/bin/agent-audit
COPY --from=heimdall-builder /usr/local/cargo/bin/heimdall /usr/local/bin/heimdall
COPY --from=prompt-builder /output/prompts ./prompts
COPY docker/codex-runner/agent-audit-run.mjs ${CODEX_RUNNER_DIR}/agent-audit-run.mjs

RUN case "${PROMPT_PROFILE}" in \
        both|open_source|closed_source) ;; \
        *) echo "invalid PROMPT_PROFILE=${PROMPT_PROFILE}" >&2; exit 2 ;; \
    esac \
    && if [ "${PROMPT_PROFILE}" = "open_source" ]; then rm -rf /opt/agent-audit/prompts/csprompt; fi \
    && if [ "${PROMPT_PROFILE}" = "closed_source" ]; then rm -rf /opt/agent-audit/prompts/osprompt; fi \
    && active_prompt="osprompt" \
    && if [ "${PROMPT_PROFILE}" = "closed_source" ]; then active_prompt="csprompt"; fi \
    && cp "/opt/agent-audit/prompts/${active_prompt}/AGENTS.md" /opt/agent-audit/AGENTS.md \
    && rm -rf /opt/agent-audit/.codex \
    && cp -a "/opt/agent-audit/prompts/${active_prompt}/.codex" /opt/agent-audit/.codex \
    && chmod +x ${CODEX_RUNNER_DIR}/agent-audit-run.mjs \
    && mkdir -p /root/.codex /opt/agent-audit/runs \
    && cp /opt/agent-audit/.codex/config.toml /root/.codex/config.toml \
    && rm -f /opt/agent-audit/.codex/config.toml

ENTRYPOINT ["node", "/opt/agent-audit/codex-runner/agent-audit-run.mjs"]

FROM runtime AS smoke-test

ARG PROMPT_PROFILE=both

RUN bash -lc 'set -euo pipefail \
    && command -v bash \
    && command -v node \
    && command -v codex \
    && command -v agent-audit \
    && command -v heimdall \
    && command -v slither \
    && command -v solc-select \
    && command -v forge \
    && command -v cast \
    && command -v anvil \
    && command -v echidna \
    && test -f /opt/agent-audit/codex-runner/agent-audit-run.mjs \
    && test -d /opt/agent-audit/codex-runner/node_modules/@openai/codex-sdk \
    && node /opt/agent-audit/codex-runner/agent-audit-run.mjs --help >/dev/null \
    && test -f /opt/agent-audit/AGENTS.md \
    && test ! -e /opt/agent-audit/.codex/config.toml \
    && test -f /root/.codex/config.toml \
    && case "${PROMPT_PROFILE}" in both) test -d /opt/agent-audit/prompts/osprompt && test -d /opt/agent-audit/prompts/csprompt ;; open_source) test -d /opt/agent-audit/prompts/osprompt && test ! -e /opt/agent-audit/prompts/csprompt ;; closed_source) test -d /opt/agent-audit/prompts/csprompt && test ! -e /opt/agent-audit/prompts/osprompt ;; esac \
    && if [ "${PROMPT_PROFILE}" = "closed_source" ]; then test -f /opt/agent-audit/.codex/skills/closed-workspace/SKILL.md && test -f /opt/agent-audit/.codex/skills/heimdall/SKILL.md; else test -f /opt/agent-audit/.codex/skills/workspace/SKILL.md; fi \
    && grep -q "^model_provider = \"apiapi\"$" /root/.codex/config.toml \
    && grep -q "^approval_policy = \"never\"$" /root/.codex/config.toml'

FROM runtime AS final
