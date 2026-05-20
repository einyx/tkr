# syntax=docker/dockerfile:1.6
# Multistage build for tkr-server.
#   Stage 1 (web):     compile the React/Vite dashboard to a single HTML file.
#   Stage 2 (builder): cargo build the Rust server, embedding the HTML via include_str!.
#   Stage 3 (runtime): copy the binary into a minimal runtime image.

ARG RUST_VERSION=1.88
ARG NODE_VERSION=20

# ---------- Stage 1: web bundle ----------
FROM node:${NODE_VERSION}-bookworm-slim AS web
WORKDIR /web

# Install deps separately so the lockfile change invalidates cache cleanly.
COPY crates/tkr-server/web/package.json crates/tkr-server/web/package-lock.json* ./
RUN npm install --no-audit --no-fund

# Build. Output goes to crates/tkr-server/static/index.html (one file,
# JS + CSS inlined via vite-plugin-singlefile). We mirror the source layout
# inside the stage so vite's outDir of `../static` lands in /static.
COPY crates/tkr-server/web ./
RUN npm run build

# ---------- Stage 2: cargo ----------
FROM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /src

RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Workspace files (everything except the web sources, which are stage 1's job).
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY filters ./filters

# Drop in the freshly-built dashboard so include_str!("../static/index.html")
# picks it up.
COPY --from=web /static/index.html ./crates/tkr-server/static/index.html

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p tkr-server \
 && cp /src/target/release/tkr-server /usr/local/bin/tkr-server

# ---------- Stage 3: runtime ----------
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 1000 tkr \
 && useradd  --system --uid 1000 --gid tkr --home-dir /home/tkr --create-home tkr \
 && mkdir -p /var/lib/tkr \
 && chown tkr:tkr /var/lib/tkr

COPY --from=builder /usr/local/bin/tkr-server /usr/local/bin/tkr-server

USER tkr
WORKDIR /home/tkr
ENV HOST=0.0.0.0 PORT=4000

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -fsS "http://127.0.0.1:${PORT:-4000}/health" >/dev/null || exit 1

EXPOSE 4000
ENTRYPOINT ["/usr/local/bin/tkr-server"]
