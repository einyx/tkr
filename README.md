# tkr

Token-optimized CLI proxy for LLM development workflows.

`tkr` filters and compresses command output before it reaches an LLM context window. It cuts 60–90% of tokens off common dev operations (build, test, git, package managers) so your AI assistant spends its context on signal, not noise.

## What's different

- **Plugin contract v2** — structured plugin lifecycle (`on_load`, `on_command_begin`, `on_line`, `on_command_end`), typed capability grants, and vault-backed storage. See `docs/superpowers/specs/2026-04-28-tkr-plugin-contract-v2-design.md` for the full spec.
- **Encrypted vault** — all plugin state lives in `~/.tkr/vault/` encrypted with age (XChaCha20-Poly1305); master key in `~/.tkr/vault/.tkr-vault.key` (0600), with one-time migration from legacy OS-keychain installs. Manage with `tkr vault {status,init,unseal,seal,rotate,export,import,audit}`.
- **Plugin architecture** — core is thin; filters and analytics are independent plugins on a shared bus (`cli.invoke` routes to plugins that declare CLI subcommands).
- **Noise analytics & suggestions** — `tkr gain`, `tkr suggest`, and `tkr watch` read vault-backed analytics. Optional **embeddings** (`cargo build --features embeddings`) improves clustering for repeated noisy lines during `tkr suggest`; without it, ranking uses signatures only.
- **Live dashboard** — `tkr watch` opens a ratatui TUI showing real-time token savings
- **Hooks** — `tkr hook claude` for Claude Code Bash hooks; `tkr hook universal` for the same JSON reply shape plus a top-level `"command"` field for other wrappers.
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

### Windows (x86_64)

Download **`tkr-x86_64-pc-windows-msvc.tar.gz`** from [Releases](https://github.com/einyx/tkr/releases). Extract `tkr.exe` using Git Bash, Windows 11 **tar**, or another tar you already use; add that directory to `PATH`, then run `tkr` from PowerShell or CMD. After a manual install, **`tkr update`** pulls the matching **`tkr-<rust-target-triple>.tar.gz`** asset from GitHub Releases.

### From source

Requires **Rust 1.88+** (see **`rust-toolchain.toml`**). Install [rustup](https://rustup.rs/), then **`rustup toolchain install 1.88.0`** (or rely on `rust-toolchain.toml` once rustup’s **`cargo`** is on **`PATH`**).

#### macOS: Homebrew hides rustup

Homebrew puts **`/opt/homebrew/bin`** first. Brew’s **`cargo`** does **not** read **`rust-toolchain.toml`** — you stay on an older **`rustc`** even inside this repo.

1. Put rustup **before** Homebrew in **`PATH`**. In **`~/.zshrc`**, **after** **`brew shellenv`**, add:

   ```sh
   export PATH="$HOME/.cargo/bin:$PATH"
   ```

2. Reload the shell (**new terminal** or **`exec zsh`**), then check:

   ```sh
   command -v cargo
   ```

   You want **`$HOME/.cargo/bin/cargo`**, not **`/opt/homebrew/bin/cargo`**.

3. Optional: **`brew unlink rust`** (or **`brew uninstall rust`**) if you installed Rust via Homebrew and no longer need it.

4. Without touching **`PATH`**, build from this repo with:

   ```sh
   ./scripts/rustup-cargo build --release
   ```

Then clone and build (use **`./scripts/rustup-cargo`** instead of **`cargo`** if **`command -v cargo`** still shows Homebrew):

```sh
git clone https://github.com/einyx/tkr
cd tkr
cargo build --release
```

Install into **`~/.cargo/bin`** from the repo root (**`--locked`** uses the workspace **`Cargo.lock`**).

If **`cargo --version`** works but **`rustc --version`** is still **1.87**, you are on **Homebrew’s `cargo`** — **`cargo install …` will always fail MSRV**. Use either:

```sh
make install
```

or:

```sh
./scripts/install-tkr
```

Both invoke **`~/.cargo/bin/cargo`** directly (same as **`./scripts/rustup-cargo install --path crates/tkr --locked --force`**).

Once **`PATH`** prefers **`~/.cargo/bin`**, plain **`cargo`** is fine:

```sh
cargo install --path crates/tkr --locked --force
```

On Unix, install from `target/release/tkr` (for example `cp target/release/tkr ~/.local/bin/`). On Windows (host triple MSVC), use `target\release\tkr.exe`. If you pass **`--target <triple>`**, the binary is under `target/<triple>/release/` (with `.exe` for Windows targets).

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

Built-in defaults match the above (`tkr-filter` only). If `~/.tkr/analytics.db` still exists from an older release, it is migrated once into `~/.tkr/vault/` and renamed to `analytics.db.migrated`.

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

Build with `--features embeddings` for vector clustering support used by `tkr suggest` when indexing noisy lines. Without it, suggestions still work using textual signatures only.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](LICENSE) or http://www.apache.org/licenses/LICENSE-2.0).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be licensed as above, without any additional terms or conditions.
