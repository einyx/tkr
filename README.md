# tkr

**Token-efficient AI dev workflows.** Filter noisy command output, query code structurally instead of reading whole files, run agents in a sandbox — Apache-2.0, self-hosted, no telemetry.

`tkr` is two complementary halves of the same problem. Your AI coding assistant burns context on two things: (1) verbose command output (build / test / git / package managers), and (2) reading whole source files to find one function. `tkr` cuts both.

## The two halves

### 1. CLI output filter (`tkr <cmd>`)

Wraps shell commands and strips dev-tool noise before it reaches the LLM context window. **60–90% token reduction** on common operations.

```sh
tkr cargo test         # only failures
tkr git status         # filtered
tkr npm install        # progress noise gone
tkr gain               # how much you've saved
tkr watch              # live dashboard (ratatui TUI)
```

Custom per-command filters live in `~/.tkr/filters/*.toml`. Three rule types: `suppress_prefix`, `suppress_regex`, `keep_regex`. See [Custom Filters](#custom-filters) below.

### 2. Code-intel MCP server (`tkr-mcp`)

Structured code-intelligence tools over MCP. Your agent reads outlines and signatures, not whole files. **1000×+ token reduction** on "find the relevant code" tasks (measured: 5.7 MB of file content collapsed to 4.4 KB of indexed results across 4 realistic questions in this repo).

| tool | what it does |
|---|---|
| `tkr_index_build` | One-shot persistent index of a repo (SQLite + tree-sitter, content-hash freshness) |
| `tkr_index_watch` | Background file watcher — index stays fresh across edits |
| `tkr_outline_file` | Symbols + line ranges for one file (no body) |
| `tkr_find_symbol` | All definitions of a name across the repo |
| `tkr_signature` | One-line declaration of a symbol |
| `tkr_read_smart` | Ranked search by free-form question → top-K symbols with locations |
| `tkr_callers_of` / `tkr_callees_of` | Cheap call-graph from the indexed `refs` table |
| `tkr_grep_summary` | Regex grep with per-file aggregation + caps |

Supports **9 languages**: Rust, Python, Go, TypeScript, JavaScript, Java, C, C++, Ruby. Set `TKR_TOON=1` for compact TOON (Token-Oriented Object Notation) tabular output — ~15% additional savings on top of the structured queries.

Wire it into Claude Code with one command:

```sh
tkr mcp install              # writes ./.mcp.json for the current repo
tkr mcp install --scope=user # or update ~/.claude.json so tkr is available in every project
tkr mcp install --print      # dry-run: show the snippet without writing
```

The index builds on first query — no manual `tkr_index_build` step needed. (Optional: call `tkr_index_watch` once if you want the index to auto-refresh on file edits across long sessions.)

Tool descriptions in tkr-mcp are written to compete directly against native `Read` / `Grep` for the agent's attention — each tool's first line says *which* native tool it should replace and the byte-cost ratio. No CLAUDE.md hint required.

### 3. (Early) sandboxed agent runtime

`tkr-agent` + `tkr-sandbox` run agents with Landlock isolation. Bring your own provider (Anthropic, OpenAI, Ollama). Less mature than (1)/(2) — usable, not yet a headline feature.

---

## What's different

- **Plugin architecture** — core is thin; filters and analytics are independent plugins on a shared bus (`cli.invoke` routes to plugins that declare CLI subcommands).
- **Encrypted vault** — all plugin state lives in `~/.tkr/vault/` encrypted with age (XChaCha20-Poly1305); master key in `~/.tkr/vault/.tkr-vault.key` (0600). One-time migration from legacy OS-keychain installs. Manage with `tkr vault {status,init,unseal,seal,rotate,export,import,audit}`.
- **Noise analytics & suggestions** — `tkr gain`, `tkr suggest`, `tkr watch` read vault-backed analytics. Optional embeddings (`--features embeddings`) improve clustering during `tkr suggest`.
- **Live dashboard** — `tkr watch` opens a ratatui TUI showing real-time token savings.
- **Hooks** — `tkr hook claude` for Claude Code Bash hooks; `tkr hook universal` for the same JSON reply shape plus a top-level `"command"` field for other wrappers.

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

### Windows (x86_64)

Download **`tkr-x86_64-pc-windows-msvc.tar.gz`** from [Releases](https://github.com/einyx/tkr/releases). Extract `tkr.exe` using Git Bash, Windows 11 **tar**, or another tar; add the directory to `PATH`, then run `tkr` from PowerShell or CMD. After a manual install, **`tkr update`** pulls the matching **`tkr-<rust-target-triple>.tar.gz`** asset.

### From source

Requires **Rust 1.88+** (see `rust-toolchain.toml`).

```sh
git clone https://github.com/einyx/tkr
cd tkr
cargo build --release
cargo install --path crates/tkr --locked --force
```

If `cargo --version` works but `rustc --version` is still 1.87, you're on Homebrew's `cargo` (it ignores `rust-toolchain.toml`). Use `make install` or `./scripts/install-tkr` — both invoke `~/.cargo/bin/cargo` directly. Or put `$HOME/.cargo/bin` ahead of `/opt/homebrew/bin` in `PATH`.

## Usage

```sh
tkr --help                   # all subcommands (incl. vault, admin)
tkr <command> [args...]      # proxy any command
tkr git status               # filtered git output
tkr cargo test               # only failures
tkr watch                    # live dashboard (run in a split pane)
tkr gain                     # token savings summary
tkr gain --breakdown         # per-command breakdown
tkr discover                 # find commands you ran without tkr
tkr vault status             # encrypted vault; plain `tkr vault` = status
tkr admin reset --plugin <name>
tkr hook universal           # stdin JSON hook (see Hooks below)
```

## Configuration

`~/.tkr/config.toml` (auto-created with sensible defaults on first use):

```toml
[core]
plugin_dir  = "~/.tkr/plugins"
socket_path = "~/.tkr/session.sock"
filter_dir  = "~/.tkr/filters"

[plugins]
chain = ["tkr-filter"]

[plugins.analytics]
db_path = "~/.tkr/analytics.db"
```

Built-in defaults match the above (`tkr-filter` only). If `~/.tkr/analytics.db` still exists from an older release, it's migrated once into `~/.tkr/vault/` and renamed to `analytics.db.migrated`.

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

## Hooks

| Command | stdin shape |
|---------|-------------|
| `tkr hook claude` | `{"tool_input":{"command":"<shell>"}}` |
| `tkr hook universal` | Claude shape **or** `{"command":"<shell>"}` at the top level |

On success both emit the same `hookSpecificOutput` JSON (Claude Code–compatible).

## Embeddings (optional)

Build with `--features embeddings` for vector clustering used by `tkr suggest` when indexing noisy lines. Without it, suggestions still work using textual signatures only.

---

## Experimental: agent mesh + on-chain payments

> **Status:** working, tested, **not** part of the v1 product pitch. Lives in the repo because the primitives may serve session sharing / multi-peer features later. If you're trying tkr for the first time, ignore this section.

`tkr` ships a peer-messaging mesh for agents (`tkr-mesh`) and a payment layer that lets agents pay each other on Base (`contracts/MeshEscrow.sol`, `contracts/JobBoard.sol`). Public broker at [tkr.prysm.sh](https://tkr.prysm.sh). Identity is a secp256k1 keypair (same shape as an Ethereum wallet); DMs are end-to-end encrypted (ECDH + AES-256-GCM); payment channels use EIP-712 receipts that match byte-for-byte between the Rust client and the Solidity contract.

```sh
tkr mesh invite-mint --slug demo \
  --broker-url wss://tkr.prysm.sh/api/v1/mesh/ws \
  --owner-key-file ~/.tkr/owner.env
tkr mesh join <invite-url>
tkr mesh tail demo
tkr pay receipt-issue ...
```

Crates: `tkr-mesh`, `tkr-server` (broker + dashboard), `tkr-model` (IPFS-backed model registry, partial), `tkr-index::bundle` (signed index distribution, protocol-complete + transport-deferred), `contracts/` (Solidity + foundry).

## License

Apache-2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be licensed as above, without any additional terms or conditions.
