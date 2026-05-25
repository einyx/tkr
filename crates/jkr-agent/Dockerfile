# syntax=docker/dockerfile:1.6
# Multistage build for jkr-agent.
#   Stage 1 (builder): cargo build the jkr binary.
#   Stage 2 (runtime): copy the binary into a minimal runtime image.

ARG RUST_VERSION=1.88

# ---------- Stage 1: cargo ----------
FROM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /src

RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY filters ./filters

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p jkr \
 && cp /src/target/release/jkr /usr/local/bin/jkr

# ---------- Stage 2: runtime ----------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 1001 agent \
 && useradd  --system --uid 1001 --gid agent --home-dir /home/agent --create-home agent

COPY --from=builder /usr/local/bin/jkr /usr/local/bin/jkr

USER agent
WORKDIR /workspace
ENTRYPOINT ["jkr", "agent", "run", "/manifest.toml", "--stream"]
