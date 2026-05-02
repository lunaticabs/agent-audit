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
COPY src ./src
COPY xtask ./xtask

RUN cargo build --release --bin agent-audit

FROM ${TOOLBOX_IMAGE}

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV CODEX_HOME=/root/.codex
ENV UV_CACHE_DIR=/tmp/uv-cache
ENV PATH=/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin
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
    && npm install -g @openai/codex \
    && apt-get purge -y --auto-remove gnupg \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /opt/agent-audit

COPY --from=builder /build/target/release/agent-audit /usr/local/bin/agent-audit
COPY AGENTS.md ./AGENTS.md
COPY .codex ./.codex
COPY docker/config.toml ./.codex/config.toml
COPY docker/entrypoint.sh /usr/local/bin/agent-audit-run

RUN chmod +x /usr/local/bin/agent-audit-run \
    && mkdir -p /root/.codex /tmp/uv-cache /opt/agent-audit/runs \
    && cp /opt/agent-audit/.codex/config.toml /root/.codex/config.toml

ENTRYPOINT ["/usr/local/bin/agent-audit-run"]
