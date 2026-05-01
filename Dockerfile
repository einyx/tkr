# syntax=docker/dockerfile:1.6
# Multistage build for tkr-server.
# Stage 1: build the release binary against the workspace.
# Stage 2: copy into a minimal runtime image.

ARG RUST_VERSION=1.88

FROM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /src

# Install build deps (k256 → uses pure Rust; aes-gcm → pure Rust;
# rusqlite is bundled — none of these need extra C libs in this set).
RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Copy the workspace.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY filters ./filters

# Cache cargo deps layer-by-layer with BuildKit's --mount=type=cache.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p tkr-server \
 && cp /src/target/release/tkr-server /usr/local/bin/tkr-server

# Stage 2: small runtime image.
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 1000 tkr \
 && useradd  --system --uid 1000 --gid tkr --home-dir /home/tkr --create-home tkr

COPY --from=builder /usr/local/bin/tkr-server /usr/local/bin/tkr-server

USER tkr
WORKDIR /home/tkr
ENV HOST=0.0.0.0 PORT=4000

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -fsS "http://127.0.0.1:${PORT:-4000}/health" >/dev/null || exit 1

EXPOSE 4000
ENTRYPOINT ["/usr/local/bin/tkr-server"]
