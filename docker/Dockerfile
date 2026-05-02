FROM docker.io/library/rust:1.88-bookworm AS builder

WORKDIR /build

COPY context/Cargo.toml context/Cargo.lock ./
COPY context/src ./src
COPY context/xtask ./xtask

RUN cargo build --release --bin agent-audit

FROM docker.io/library/debian:bookworm-slim

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV CODEX_HOME=/root/.codex
ENV UV_CACHE_DIR=/tmp/uv-cache
ENV PATH=/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin
ARG ECHIDNA_VERSION=2.3.2
ARG FOUNDRY_VERSION=1.7.0

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        nodejs \
        npm \
        python3 \
        python3-pip \
        tar \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g @openai/codex \
    && pip3 install --no-cache-dir slither-analyzer solc-select \
    && curl -fsSL "https://github.com/foundry-rs/foundry/releases/download/v${FOUNDRY_VERSION}/foundry_v${FOUNDRY_VERSION}_linux_amd64.tar.gz" -o /tmp/foundry.tar.gz \
    && tar -xzf /tmp/foundry.tar.gz -C /usr/local/bin \
    && curl -fsSL "https://github.com/crytic/echidna/releases/download/v${ECHIDNA_VERSION}/echidna-${ECHIDNA_VERSION}-x86_64-linux.tar.gz" -o /tmp/echidna.tar.gz \
    && tar -xzf /tmp/echidna.tar.gz -C /usr/local/bin \
    && rm -f /tmp/foundry.tar.gz /tmp/echidna.tar.gz

WORKDIR /opt/agent-audit

COPY --from=builder /build/target/release/agent-audit /usr/local/bin/agent-audit
COPY context/AGENTS.md ./AGENTS.md
COPY context/.codex ./.codex
COPY entrypoint.sh /usr/local/bin/agent-audit-run

RUN chmod +x /usr/local/bin/agent-audit-run \
    && mkdir -p /root/.codex /tmp/uv-cache /opt/agent-audit/runs \
    && cp /opt/agent-audit/.codex/config.toml /root/.codex/config.toml

ENTRYPOINT ["/usr/local/bin/agent-audit-run"]
