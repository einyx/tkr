# tkr

Token-optimized CLI proxy for LLM development workflows.

`tkr` filters and compresses command output before it reaches an LLM context window. It cuts 60–90% of tokens off common dev operations (build, test, git, package managers) so your AI assistant spends its context on signal, not noise.

## What's different

- **Plugin contract v2** — structured plugin lifecycle (`on_load`, `on_command_begin`, `on_line`, `on_command_end`), typed capability grants, and vault-backed storage. See `docs/superpowers/specs/2026-04-28-tkr-plugin-contract-v2-design.md` for the full spec.
- **Encrypted vault** — all plugin state lives in `~/.tkr/vault/` encrypted with age (XChaCha20-Poly1305), master key in the OS keychain. Manage with `tkr vault {status,init,unseal,seal,rotate,export,import,audit}`.
- **Plugin architecture** — core is thin; filters, semantic dedup, and analytics are independent plugins
- **Semantic deduplication** — collapses near-duplicate output lines using local embeddings (Ollama or exact-match fallback)
- **Relevance scoring** — drops low-signal lines that don't relate to the command intent
- **Live dashboard** — `tkr watch` opens a ratatui TUI showing real-time token savings
- **Apache-2.0 licensed** — no telemetry, no upstream

## Install

### Homebrew (macOS / Linux)

```sh
brew tap einyx/tkr
brew install tkr
```

### Curl-to-bash

```sh
curl -fsSL https://raw.githubusercontent.com/einyx/tkr/main/install.sh | bash
```

### From source

```sh
git clone https://github.com/einyx/tkr
cd tkr
cargo build --release
cp target/release/tkr ~/.local/bin/
```

## Usage

```sh
tkr <command> [args...]      # proxy any command
tkr git status               # filtered git output
tkr cargo test               # only failures
tkr watch                    # live dashboard (run in a split pane)
tkr gain                     # token savings summary
tkr gain --breakdown         # per-command breakdown
tkr discover                 # find commands you ran without tkr
```

## Configuration

`~/.tkr/config.toml` (auto-created with sensible defaults on first use):

```toml
[core]
plugin_dir  = "~/.tkr/plugins"
socket_path = "~/.tkr/session.sock"
filter_dir  = "~/.tkr/filters"

[plugins]
chain = ["tkr-filter", "tkr-semantic", "tkr-analytics"]

[plugins.semantic]
dedup_threshold     = 0.92
relevance_threshold = 0.15
window_size         = 50
emit_summaries      = true
ollama_url          = "http://localhost:11434"
ollama_model        = "nomic-embed-text"

[plugins.analytics]
db_path = "~/.tkr/analytics.db"
```

## Custom Filters

Drop a TOML filter file in `~/.tkr/filters/`:

```toml
command = "mytool"

[[rules]]
type   = "suppress_regex"
pattern = "^DEBUG: "

[[rules]]
type    = "suppress_prefix"
prefix  = "info:"

[[rules]]
type    = "keep_regex"
pattern = "^(error|warning):"
```

Three rule types: `suppress_prefix`, `suppress_regex`, `keep_regex` (drops anything not matching).

## Semantic Backend

`tkr-semantic` resolves the embedding backend in this order:

1. **Ollama** at `http://localhost:11434` — needs `ollama pull nomic-embed-text`
2. **Exact-match dedup** — fallback, no embeddings; still suppresses duplicate lines

If you run Ollama locally, semantic dedup also enables relevance scoring (drops lines unrelated to the command intent).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be licensed as above, without any additional terms or conditions.
