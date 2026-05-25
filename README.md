# jkr

**Token-efficient AI dev workflows.** Filter noisy command output, query code structurally instead of reading whole files, run agents in a sandbox — Apache-2.0, self-hosted, no telemetry.

`jkr` is two complementary halves of the same problem. Your AI coding assistant burns context on two things: (1) verbose command output (build / test / git / package managers), and (2) reading whole source files to find one function. `jkr` cuts both.

## The two halves

### 1. CLI output filter (`jkr <cmd>`)

Wraps shell commands and strips dev-tool noise before it reaches the LLM context window. **60–90% token reduction** on common operations.

```sh
jkr cargo test         # only failures
jkr git status         # filtered
jkr npm install        # progress noise gone
jkr gain               # how much you've saved
jkr watch              # live dashboard (ratatui TUI)
```

Custom per-command filters live in `~/.jkr/filters/*.toml`. Three rule types: `suppress_prefix`, `suppress_regex`, `keep_regex`. See [Custom Filters](#custom-filters) below.

### 2. Code-intel MCP server (`jkr-mcp`)

Structured code-intelligence tools over MCP. Your agent reads outlines and signatures, not whole files. **1000×+ token reduction** on "find the relevant code" tasks (measured: 5.7 MB of file content collapsed to 4.4 KB of indexed results across 4 realistic questions in this repo).

| tool | what it does |
|---|---|
| `jkr_index_build` | One-shot persistent index of a repo (SQLite + tree-sitter, content-hash freshness) |
| `jkr_index_watch` | Background file watcher — index stays fresh across edits |
| `jkr_outline_file` | Symbols + line ranges for one file (no body) |
| `jkr_find_symbol` | All definitions of a name across the repo |
| `jkr_signature` | One-line declaration of a symbol |
| `jkr_read_smart` | Ranked search by free-form question → top-K symbols with locations |
| `jkr_callers_of` / `jkr_callees_of` | Cheap call-graph from the indexed `refs` table |
| `jkr_grep_summary` | Regex grep with per-file aggregation + caps |

Supports **9 languages**: Rust, Python, Go, TypeScript, JavaScript, Java, C, C++, Ruby. Set `JKR_TOON=1` for compact TOON (Token-Oriented Object Notation) tabular output — ~15% additional savings on top of the structured queries.

Wire it into Claude Code with one command:

```sh
jkr mcp install              # writes ./.mcp.json for the current repo
jkr mcp install --scope=user # or update ~/.claude.json so jkr is available in every project
jkr mcp install --print      # dry-run: show the snippet without writing
```

The index builds on first query — no manual `jkr_index_build` step needed. (Optional: call `jkr_index_watch` once if you want the index to auto-refresh on file edits across long sessions.)

Tool descriptions in jkr-mcp are written to compete directly against native `Read` / `Grep` for the agent's attention — each tool's first line says *which* native tool it should replace and the byte-cost ratio. No CLAUDE.md hint required.

### 3. (Early) sandboxed agent runtime

`jkr-agent` + `jkr-sandbox` run agents with Landlock isolation. Bring your own provider (Anthropic, OpenAI, Ollama). Less mature than (1)/(2) — usable, not yet a headline feature.

---

## What's different

- **Plugin architecture** — core is thin; filters and analytics are independent plugins on a shared bus (`cli.invoke` routes to plugins that declare CLI subcommands).
- **Encrypted vault** — all plugin state lives in `~/.jkr/vault/` encrypted with age (XChaCha20-Poly1305); master key in `~/.jkr/vault/.jkr-vault.key` (0600). One-time migration from legacy OS-keychain installs. Manage with `jkr vault {status,init,unseal,seal,rotate,export,import,audit}`.
- **Noise analytics & suggestions** — `jkr gain`, `jkr suggest`, `jkr watch` read vault-backed analytics. Optional embeddings (`--features embeddings`) improve clustering during `jkr suggest`.
- **Live dashboard** — `jkr watch` opens a ratatui TUI showing real-time token savings.
- **Hooks** — `jkr hook claude` for Claude Code Bash hooks; `jkr hook universal` for the same JSON reply shape plus a top-level `"command"` field for other wrappers.

## Install

### Homebrew (macOS / Linux)

```sh
brew tap einyx/jkr
brew install jkr
```

### Curl-to-bash

```sh
curl -fsSL https://raw.githubusercontent.com/einyx/jkr/main/install.sh | bash
```

### Windows (x86_64)

Download **`jkr-x86_64-pc-windows-msvc.tar.gz`** from [Releases](https://github.com/einyx/jkr/releases). Extract `jkr.exe` using Git Bash, Windows 11 **tar**, or another tar; add the directory to `PATH`, then run `jkr` from PowerShell or CMD. After a manual install, **`jkr update`** pulls the matching **`jkr-<rust-target-triple>.tar.gz`** asset.

### From source

Requires **Rust 1.88+** (see `rust-toolchain.toml`).

```sh
git clone https://github.com/einyx/jkr
cd jkr
cargo build --release
cargo install --path crates/jkr --locked --force
```

If `cargo --version` works but `rustc --version` is still 1.87, you're on Homebrew's `cargo` (it ignores `rust-toolchain.toml`). Use `make install` or `./scripts/install-jkr` — both invoke `~/.cargo/bin/cargo` directly. Or put `$HOME/.cargo/bin` ahead of `/opt/homebrew/bin` in `PATH`.

## Usage

```sh
jkr --help                   # all subcommands (incl. vault, admin)
jkr <command> [args...]      # proxy any command
jkr git status               # filtered git output
jkr cargo test               # only failures
jkr watch                    # live dashboard (run in a split pane)
jkr gain                     # token savings summary
jkr gain --breakdown         # per-command breakdown
jkr discover                 # find commands you ran without jkr
jkr vault status             # encrypted vault; plain `jkr vault` = status
jkr admin reset --plugin <name>
jkr hook universal           # stdin JSON hook (see Hooks below)
```

## Configuration

`~/.jkr/config.toml` (auto-created with sensible defaults on first use):

```toml
[core]
plugin_dir  = "~/.jkr/plugins"
socket_path = "~/.jkr/session.sock"
filter_dir  = "~/.jkr/filters"

[plugins]
chain = ["jkr-filter"]

[plugins.analytics]
db_path = "~/.jkr/analytics.db"
```

Built-in defaults match the above (`jkr-filter` only). If `~/.jkr/analytics.db` still exists from an older release, it's migrated once into `~/.jkr/vault/` and renamed to `analytics.db.migrated`.

## Custom Filters

Drop a TOML filter file in `~/.jkr/filters/`:

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

## Hooks

| Command | stdin shape |
|---------|-------------|
| `jkr hook claude` | `{"tool_input":{"command":"<shell>"}}` |
| `jkr hook universal` | Claude shape **or** `{"command":"<shell>"}` at the top level |

On success both emit the same `hookSpecificOutput` JSON (Claude Code–compatible).

## Embeddings (optional)

Build with `--features embeddings` for vector clustering used by `jkr suggest` when indexing noisy lines. Without it, suggestions still work using textual signatures only.

---

## Experimental: agent mesh + on-chain payments

> **Status:** working, tested, **not** part of the v1 product pitch. Lives in the repo because the primitives may serve session sharing / multi-peer features later. If you're trying jkr for the first time, ignore this section.

`jkr` ships a peer-messaging mesh for agents (`jkr-mesh`) and a payment layer that lets agents pay each other on Base (`contracts/MeshEscrow.sol`, `contracts/JobBoard.sol`). Public broker at [tkr.prysm.sh](https://tkr.prysm.sh). Identity is a secp256k1 keypair (same shape as an Ethereum wallet); DMs are end-to-end encrypted (ECDH + AES-256-GCM); payment channels use EIP-712 receipts that match byte-for-byte between the Rust client and the Solidity contract.

```sh
jkr mesh invite-mint --slug demo \
  --broker-url wss://tkr.prysm.sh/api/v1/mesh/ws \
  --owner-key-file ~/.jkr/owner.env
jkr mesh join <invite-url>
jkr mesh tail demo
jkr pay receipt-issue ...
```

Crates: `jkr-mesh`, `jkr-server` (broker + dashboard), `jkr-model` (IPFS-backed model registry, partial), `jkr-index::bundle` (signed index distribution, protocol-complete + transport-deferred), `contracts/` (Solidity + foundry).

## License

Apache-2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be licensed as above, without any additional terms or conditions.
