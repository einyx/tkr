# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & test

`rust-toolchain.toml` pins Rust 1.88.0. Use rustup's cargo (`~/.cargo/bin/cargo`), not Homebrew's — the Makefile encodes this explicitly because Homebrew's cargo ignores `rust-toolchain.toml` and fails MSRV on `darling`/`time`/`instability`.

```sh
cargo build --release -p jkr              # ship binary
cargo test --release -p <crate> --lib     # unit tests for one crate
cargo test --release                      # full workspace (slow; jkr-server's persistence/mesh_e2e tests need Postgres/Redis/a broker)
make build                                # same as cargo build, with rustup-cargo guard
```

For per-test debugging: `cargo test -p <crate> <test_name_substring> -- --nocapture`.

Token-cost measurement: `cargo run -p jkr-mcp --release --example bench_token_cost -- <repo>` runs every index-backed MCP tool against a representative query set and prints per-tool byte counts. Current baseline on this repo: ~9KB / ~2.5K tokens for a 17-call session. Run before/after any shape change in `crates/jkr-mcp/src/index_backed.rs` to confirm direction.

The Makefile's `publish-darwin` is hardcoded to Apple Silicon paths (`RUSTUP := $(HOME)/.rustup/toolchains/1.88.0-aarch64-apple-darwin/bin`); on Intel Mac, invoke cargo directly instead. Release flow ships via `scripts/release.sh` → `make publish`, which `uname -s`-routes to `publish-darwin` (Mac) or `publish-linux` (Linux); only `publish-darwin` calls `_bump-tap` to push the homebrew formula update.

Web/contracts/mesh-devnet targets live in `Makefile` too — `contracts-test`, `anvil-fork`, `deploy-local`, `demo-payment`, `deploy-mesh`, `web-{install,dev,build}`. These exist because the JobBoard marketplace and mesh broker are part of the workspace.

## Architecture

`jkr` is a Cargo workspace solving two distinct token-cost problems. They share infrastructure but ship as one binary with subcommands:

```
crates/
  jkr/            CLI entry — clap subcommands dispatch to everything else
  jkr-filter/     Per-command output filters (~/.jkr/filters/*.toml rule packs)
  jkr-mcp/        MCP server for code-intel tools — the "structured query" half
  jkr-index/      Tree-sitter + SQLite persistent code index (the data jkr-mcp queries)
  jkr-sandbox/    Landlock (Linux) / sandbox-exec (macOS) child execution
  jkr-server/     Web dashboard + LLM proxy + receipt store (workspace member; not in the shipped release artifact)
  jkr-mesh/       p2p mesh — broker enrollment, payments, JobBoard contract calls
  jkr-providers/  Anthropic/Ollama API adapters (one schema, two wire formats)
  jkr-agent/      Agent loop primitives (used by jkr-server's gateway)
  jkr-analytics/  Token-savings rollup (powers `jkr gain` and `jkr watch`)
  jkr-session-recorder/  Session capture for replay
  jkr-model/      llama.cpp model distribution (IPFS-backed)
  jkr-api/        Shared types between jkr-server and jkr (DTOs)
```

### The two halves of token saving

**Half 1: output filter** (`jkr <cmd>`). User runs `jkr cargo test`; jkr execs `cargo test`, streams stdout/stderr through a filter pack matched on the program name (`cargo`, `git`, `npm`, …), then passes the survivors to the parent terminal. Rule types: `suppress_prefix`, `suppress_regex`, `keep_regex` — applied in TOML-defined order. Filter packs live in `~/.jkr/filters/*.toml`; defaults ship in `jkr-filter/assets/`. The savings tally goes to `jkr-analytics`, visible via `jkr gain` / `jkr watch`.

**Half 2: code-intel MCP** (`jkr mcp` over stdio). Agent calls structured tools instead of `Read`-ing whole files. The data layer is `jkr-index`: a tree-sitter walk emits a SQLite DB (`.jkr/index.db`) with `files`, `symbols`, `refs` tables + FTS5 virtual table. Content-hash freshness check; `jkr_index_watch` runs a `notify`-based background watcher so the index stays current across edits without re-walking. Nine languages: rust, python, go, ts, js, java, c, c++, ruby (all via `tree-sitter-*` 0.23). `refs.to_name` is the **unresolved** callee identifier — call-graph queries match on that text, which is intentional (cross-file unresolved-name matching is cheaper than building a real semantic graph and good enough for 1000×-ish savings on "where is X called?" questions).

Top-level MCP tool families:
- `jkr_index_build` / `jkr_index_watch` — index lifecycle
- `jkr_outline_file` / `jkr_find_symbol` / `jkr_signature` — symbol lookup
- `jkr_read_smart` — FTS-ranked free-form question → top-K symbols
- `jkr_callers_of` / `jkr_callees_of` — direct call-graph (1 hop) via `refs` table
- `jkr_call_path` — shortest transitive path between two symbols (BFS, bounded depth, cycle-safe)
- `jkr_grep_summary` — regex grep with per-file aggregation + caps
- `jkr_jobs_list` / `jkr_mesh_status` — read-only views into the JobBoard contract and mesh broker state

**Response shape conventions** (locked in by `response_shapes_stay_tight` + `path_dedup_*` tests in `index_backed.rs`):
- No column padding (`{:<8} {:<40}` was burning ~30-40 chars/row for visual alignment a tokenizer doesn't use).
- No didactic footers (agents already on a `jkr_*` call know how to use `Read`).
- Path-dedup: when a response has ≥3 rows and at least one path repeats, an `@P` header lists each unique path once and rows reference paths by short ID (`@1:line`, `@2:line`). Agents must parse `@N:` prefixes the same way as `path:` prefixes. Implementation: `maybe_path_table()` in `index_backed.rs`.

`JKR_TOON=1` switches MCP tool responses to TOON (Token-Oriented Object Notation) — tabular shape, ~15% smaller than the JSON. Code lives in `jkr-mcp/src/toon.rs`. TOON path is independent of the plain-text path-dedup; agents typically use one or the other.

### Sandbox

`jkr-sandbox` wraps `Command` execution with platform-specific isolation:
- Linux: Landlock V4 (kernel 6.7+) for fs scoping + network rules.
- macOS: `sandbox-exec` with a generated SBPL profile. The default profile must include baseline allows for `file-read-metadata`, `file-map-executable`, `ipc-posix-shm`, `process-info* (target self)`, `mach-priv-host-port`, `mach-register` — without these, `(deny default)` SIGKILLs the child during dyld image load and the parent sees a silent exit (the v0.3.0 bug fixed in #14).

`jkr sandbox run -- <cmd>` is the user-facing wrapper. `jkr sandbox claude` is the agent-friendly preset: cwd is the only writable path, `~/.claude` is read-only, auth/locale env vars forwarded. The sandbox crate's `spawn_and_collect` reader-threads enforce a configurable byte cap (default 16 MiB total, split per-stream) and a wall-clock timeout.

### LLM proxy / mesh

`jkr-server` is the gateway: `/v1/messages` (Anthropic) and `/v1/chat/completions` (OpenAI) proxies with concurrency caps, pre-flight `RedactionEngine` (AWS keys, GitHub PATs, OpenAI/Anthropic keys, JWTs), `SseRewriter` for streaming response scrubbing, and an `InjectionEngine` heuristic for prompt-injection patterns. It is a workspace member but **not** part of `make publish`'s release artifact, so the shipped CLI doesn't link its deps. Re-adding it to the workspace pulls sqlx 0.8.0 + rustls 0.21 / rustls-webpki 0.101 into the shared `Cargo.lock` (dependabot will flag these); sqlx can't be bumped past 0.8.0 because its newer `libsqlite3-sys` conflicts with the `rusqlite 0.31` (`links = "sqlite3"`) that `jkr-index` pins. The upstream-proxy and token-exchange paths use ureq 3.x (`Agent::config_builder().http_status_as_error(false)` to read 4xx/5xx bodies; `into_body().read_to_string()` / `read_to_vec()` for response bodies).

`jkr-mesh` does broker enrollment via HTTPS POST to `/join` then upgrades to WebSocket for the live channel. `MESH_WS_COOKIE` env var gates the join (and the WS upgrade). The mesh registry on-chain is the JobBoard contract (`contracts/`) — Solidity, deployed via `anvil` locally or testnet.

## Conventions worth knowing

- **No comment rot**: don't reference the current task / PR / fix in code comments. Belongs in commit messages and PR descriptions.
- **No co-author trailers in commits** — author preference, enforced for this repo.
- **Tests for `jkr-sandbox`'s macOS path are gated `#[cfg(target_os = "macos")]`** — they don't run on Linux CI. The string-level tests check generated SBPL contents; the end-to-end test invokes real `sandbox-exec` on `/bin/echo`. The latter is the one that catches "child dies during dyld" — add it for any new platform-specific sandbox feature.
- **Per-language tree-sitter wiring** lives in `crates/jkr-index/src/lang.rs`. Adding a language: add the `tree-sitter-<lang>` dep in `Cargo.toml`, register a `LangConfig` entry mapping file extensions to the `tree_sitter::Language` and the symbol-kind capture query.

## Useful env vars

| var | purpose |
|---|---|
| `JKR_TOON=1` | switch MCP tool responses to TOON tabular format |
| `JKR_MCP_ROOT` | pin the index repo root (default: cwd) |
| `JKR_BIN` | override the `jkr` binary path used by `jkr-mcp` for spawning subcommands |
| `JKR_MESH_HOST` | broker host for `jkr_mesh_status` MCP tool |
| `JKR_JOB_BOARD` / `JKR_JOB_RPC_URL` | override JobBoard contract address + RPC for `jkr_jobs_list` |
| `JKR_MESH_WS_COOKIE` | session cookie forwarded on broker enrollment + WS upgrade |
| `JKR_INGEST_URL` / `JKR_INGEST_TOKEN` | sandbox-run telemetry POST target (silent on any failure) |
| `JKR_MAC_HOST` / `JKR_MAC_ROOT` | `jarvis jkr publish-mac` SSH target (default: `alessio@192.168.190.242:~/jkr`) |
