ARG RUST_VERSION=1.88.0

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

RUN cargo build --release -p agent-audit-dispatcher

FROM docker.io/library/ubuntu:22.04 AS runtime

USER root

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/agent-audit-dispatcher /usr/local/bin/agent-audit-dispatcher

ENTRYPOINT ["/usr/local/bin/agent-audit-dispatcher"]

FROM runtime AS smoke-test

RUN /usr/local/bin/agent-audit-dispatcher --help >/dev/null

FROM runtime AS final
