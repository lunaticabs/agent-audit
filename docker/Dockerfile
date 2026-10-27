ARG IMAGE_PLATFORM=linux/amd64
ARG TOOLBOX_IMAGE=ghcr.io/trailofbits/eth-security-toolbox/ci:nightly-20261026@sha256:001961901a572eafbd476dd405e2f3b972a17653d653dd779c6383cd1a48180c
ARG HEIMDALL_REV=13ae7cb02959feca08bcaafb10c9d66a28037f77
ARG HEIMDALL_RUST_VERSION=1.91.0
ARG HEIMDALL_CARGO_BUILD_JOBS=1
ARG NODE_MAJOR=24
ARG DSH_VERSION=0.2.1-rc.3
ARG FORGE_STD_REV=9964d28dd139cbceb8a7c7c3e182c29cd9888bfd

FROM --platform=${IMAGE_PLATFORM} docker.io/library/ubuntu:22.04 AS heimdall-builder

USER root

ENV DEBIAN_FRONTEND=noninteractive
ENV PATH=/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
ARG HEIMDALL_REV
ARG HEIMDALL_RUST_VERSION
ARG HEIMDALL_CARGO_BUILD_JOBS

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
ENV AGENT_AUDIT_PROJECT_ROOT=/opt/agent-audit
ENV AGENT_AUDIT_RUNS_DIR=/opt/agent-audit/runs
ENV AGENT_AUDIT_STATS_MODULE=/opt/agent-audit/tools
ENV DSH_HOME=/opt/agent-audit/dsh-home
ENV DSH_PERMISSION_MODE=danger-full-access
ENV DSH_TELEMETRY_DISABLED=1
ENV FOUNDRY_DISABLE_NIGHTLY_WARNING=1

ARG NODE_MAJOR
ARG DSH_VERSION
ARG FORGE_STD_REV

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        build-essential \
        ca-certificates \
        curl \
        git \
        gnupg \
        jq \
        less \
        procps \
        python3 \
        ripgrep \
        unzip \
        xz-utils \
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

# DeepSeek Harness runs the audit loop. Install scripts are needed for its
# native spawn/pty helpers; they run only during this trusted image build.
RUN npm install -g "@deepseek-ai/dsh@${DSH_VERSION}" \
        --dangerously-allow-all-scripts \
        --no-audit \
        --no-fund \
    && npm cache clean --force

RUN mkdir -p /opt/agent-audit/vendor/forge-std \
    && cd /opt/agent-audit/vendor/forge-std \
    && git init \
    && git remote add origin https://github.com/foundry-rs/forge-std.git \
    && git fetch --depth 1 origin "${FORGE_STD_REV}" \
    && git checkout --detach FETCH_HEAD \
    && git submodule update --init --depth 1 \
    && rm -rf .git

WORKDIR /opt/agent-audit

COPY --from=heimdall-builder /root/.cargo/bin/heimdall /usr/local/bin/heimdall
COPY docker/entrypoint.sh ./entrypoint.sh
COPY docker/dsh/cordis.patch.yml ./dsh-home/cordis.patch.yml
COPY docker/dsh/skills ./dsh-home/skills
# Post-run statistics: the entrypoint derives runs/<run_id>/stats/metrics.json
# from the finished run directory. Stdlib only; never changes the audit result.
COPY tools/audit_stats ./tools/audit_stats

RUN chmod +x /opt/agent-audit/entrypoint.sh \
    && mkdir -p /opt/agent-audit/runs /opt/agent-audit/dsh-home

ENTRYPOINT ["/opt/agent-audit/entrypoint.sh"]

FROM runtime AS smoke-test

RUN bash -lc 'set -euo pipefail \
    && command -v bash \
    && command -v node \
    && command -v dsh \
    && command -v jq \
    && command -v rg \
    && command -v slither \
    && command -v solc-select \
    && command -v forge \
    && command -v cast \
    && command -v anvil \
    && command -v echidna \
    && command -v heimdall \
    && node --version | grep -Eq "^v24\." \
    && dsh --version \
    && node --expose-internals "$(readlink -f "$(command -v dsh)")" --profile headless --help >/dev/null \
    && /opt/agent-audit/entrypoint.sh --help >/dev/null 2>&1 \
    && heimdall --help >/dev/null \
    && test -f /opt/agent-audit/vendor/forge-std/src/Test.sol \
    && test -f /opt/agent-audit/dsh-home/cordis.patch.yml \
    && test -f /opt/agent-audit/dsh-home/skills/evm-audit/SKILL.md \
    && test -f /opt/agent-audit/dsh-home/skills/evm-audit/references/tools.md \
    && test -f /opt/agent-audit/dsh-home/skills/evm-audit/references/report.md \
    && test -f /opt/agent-audit/dsh-home/skills/evm-audit/references/chains.md \
    && PYTHONPATH=/opt/agent-audit/tools python3 -m audit_stats --version'

FROM runtime AS final
