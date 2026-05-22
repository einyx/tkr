# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & test

`rust-toolchain.toml` pins Rust 1.88.0. Use rustup's cargo (`~/.cargo/bin/cargo`), not Homebrew's — the Makefile encodes this explicitly because Homebrew's cargo ignores `rust-toolchain.toml` and fails MSRV on `darling`/`time`/`instability`.

```sh
cargo build --release -p tkr              # ship binary
cargo test --release -p <crate> --lib     # unit tests for one crate
cargo test --release                      # full workspace (slow; tkr-server may be broken — see below)
make build                                # same as cargo build, with rustup-cargo guard
```

For per-test debugging: `cargo test -p <crate> <test_name_substring> -- --nocapture`.

The Makefile's `publish-darwin` is hardcoded to Apple Silicon paths (`RUSTUP := $(HOME)/.rustup/toolchains/1.88.0-aarch64-apple-darwin/bin`); on Intel Mac, invoke cargo directly instead. Release flow ships via `scripts/release.sh` → `make publish`, which `uname -s`-routes to `publish-darwin` (Mac) or `publish-linux` (Linux); only `publish-darwin` calls `_bump-tap` to push the homebrew formula update.

Web/contracts/mesh-devnet targets live in `Makefile` too — `contracts-test`, `anvil-fork`, `deploy-local`, `demo-payment`, `deploy-mesh`, `web-{install,dev,build}`. These exist because the JobBoard marketplace and mesh broker are part of the workspace.

## Architecture

`tkr` is a Cargo workspace solving two distinct token-cost problems. They share infrastructure but ship as one binary with subcommands:

```
crates/
  tkr/            CLI entry — clap subcommands dispatch to everything else
  tkr-filter/     Per-command output filters (~/.tkr/filters/*.toml rule packs)
  tkr-mcp/        MCP server for code-intel tools — the "structured query" half
  tkr-index/      Tree-sitter + SQLite persistent code index (the data tkr-mcp queries)
  tkr-sandbox/    Landlock (Linux) / sandbox-exec (macOS) child execution
  tkr-server/     Web dashboard + LLM proxy + receipt store (currently broken on main; see below)
  tkr-mesh/       p2p mesh — broker enrollment, payments, JobBoard contract calls
  tkr-providers/  Anthropic/Ollama API adapters (one schema, two wire formats)
  tkr-agent/      Agent loop primitives (used by tkr-server's gateway)
  tkr-analytics/  Token-savings rollup (powers `tkr gain` and `tkr watch`)
  tkr-session-recorder/  Session capture for replay
  tkr-model/      llama.cpp model distribution (IPFS-backed)
  tkr-api/        Shared types between tkr-server and tkr (DTOs)
```

### The two halves of token saving

**Half 1: output filter** (`tkr <cmd>`). User runs `tkr cargo test`; tkr execs `cargo test`, streams stdout/stderr through a filter pack matched on the program name (`cargo`, `git`, `npm`, …), then passes the survivors to the parent terminal. Rule types: `suppress_prefix`, `suppress_regex`, `keep_regex` — applied in TOML-defined order. Filter packs live in `~/.tkr/filters/*.toml`; defaults ship in `tkr-filter/assets/`. The savings tally goes to `tkr-analytics`, visible via `tkr gain` / `tkr watch`.

**Half 2: code-intel MCP** (`tkr mcp` over stdio). Agent calls structured tools instead of `Read`-ing whole files. The data layer is `tkr-index`: a tree-sitter walk emits a SQLite DB (`.tkr/index.db`) with `files`, `symbols`, `refs` tables + FTS5 virtual table. Content-hash freshness check; `tkr_index_watch` runs a `notify`-based background watcher so the index stays current across edits without re-walking. Nine languages: rust, python, go, ts, js, java, c, c++, ruby (all via `tree-sitter-*` 0.23). `refs.to_name` is the **unresolved** callee identifier — call-graph queries match on that text, which is intentional (cross-file unresolved-name matching is cheaper than building a real semantic graph and good enough for 1000×-ish savings on "where is X called?" questions).

Top-level MCP tool families:
- `tkr_index_build` / `tkr_index_watch` — index lifecycle
- `tkr_outline_file` / `tkr_find_symbol` / `tkr_signature` — symbol lookup
- `tkr_read_smart` — FTS-ranked free-form question → top-K symbols
- `tkr_callers_of` / `tkr_callees_of` — direct call-graph via `refs` table
- `tkr_grep_summary` — regex grep with per-file aggregation + caps
- `tkr_jobs_list` / `tkr_mesh_status` — read-only views into the JobBoard contract and mesh broker state

`TKR_TOON=1` switches MCP tool responses to TOON (Token-Oriented Object Notation) — tabular shape, ~15% smaller than the JSON. Code lives in `tkr-mcp/src/toon.rs`.

### Sandbox

`tkr-sandbox` wraps `Command` execution with platform-specific isolation:
- Linux: Landlock V4 (kernel 6.7+) for fs scoping + network rules.
- macOS: `sandbox-exec` with a generated SBPL profile. The default profile must include baseline allows for `file-read-metadata`, `file-map-executable`, `ipc-posix-shm`, `process-info* (target self)`, `mach-priv-host-port`, `mach-register` — without these, `(deny default)` SIGKILLs the child during dyld image load and the parent sees a silent exit (the v0.3.0 bug fixed in #14).

`tkr sandbox run -- <cmd>` is the user-facing wrapper. `tkr sandbox claude` is the agent-friendly preset: cwd is the only writable path, `~/.claude` is read-only, auth/locale env vars forwarded. The sandbox crate's `spawn_and_collect` reader-threads enforce a configurable byte cap (default 16 MiB total, split per-stream) and a wall-clock timeout.

### LLM proxy / mesh (currently disabled paths)

`tkr-server` is the gateway: `/v1/messages` (Anthropic) and `/v1/chat/completions` (OpenAI) proxies with concurrency caps, pre-flight `RedactionEngine` (AWS keys, GitHub PATs, OpenAI/Anthropic keys, JWTs), `SseRewriter` for streaming response scrubbing, and an `InjectionEngine` heuristic for prompt-injection patterns. **`tkr-server` does not currently compile against ureq 3.x** (3 token-exchange sites + `response_body_to_bytes` helper need the migration that `tkr-providers`/`tkr-mesh`/`tkr` got in #15). It is not part of `make publish`'s release artifact, so the CLI ships unblocked.

`tkr-mesh` does broker enrollment via HTTPS POST to `/join` then upgrades to WebSocket for the live channel. `MESH_WS_COOKIE` env var gates the join (and the WS upgrade). The mesh registry on-chain is the JobBoard contract (`contracts/`) — Solidity, deployed via `anvil` locally or testnet.

## Conventions worth knowing

- **No comment rot**: don't reference the current task / PR / fix in code comments. Belongs in commit messages and PR descriptions.
- **No co-author trailers in commits** — author preference, enforced for this repo.
- **Tests for `tkr-sandbox`'s macOS path are gated `#[cfg(target_os = "macos")]`** — they don't run on Linux CI. The string-level tests check generated SBPL contents; the end-to-end test invokes real `sandbox-exec` on `/bin/echo`. The latter is the one that catches "child dies during dyld" — add it for any new platform-specific sandbox feature.
- **Per-language tree-sitter wiring** lives in `crates/tkr-index/src/lang.rs`. Adding a language: add the `tree-sitter-<lang>` dep in `Cargo.toml`, register a `LangConfig` entry mapping file extensions to the `tree_sitter::Language` and the symbol-kind capture query.

## Useful env vars

| var | purpose |
|---|---|
| `TKR_TOON=1` | switch MCP tool responses to TOON tabular format |
| `TKR_MCP_ROOT` | pin the index repo root (default: cwd) |
| `TKR_BIN` | override the `tkr` binary path used by `tkr-mcp` for spawning subcommands |
| `TKR_MESH_HOST` | broker host for `tkr_mesh_status` MCP tool |
| `TKR_JOB_BOARD` / `TKR_JOB_RPC_URL` | override JobBoard contract address + RPC for `tkr_jobs_list` |
| `TKR_MESH_WS_COOKIE` | session cookie forwarded on broker enrollment + WS upgrade |
| `TKR_INGEST_URL` / `TKR_INGEST_TOKEN` | sandbox-run telemetry POST target (silent on any failure) |
| `TKR_MAC_HOST` / `TKR_MAC_ROOT` | `jarvis tkr publish-mac` SSH target (default: `alessio@192.168.190.242:~/tkr`) |
