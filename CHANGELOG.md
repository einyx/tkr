# Changelog

All notable changes to tkr are documented here.

## [Unreleased]

### CI and releases

- GitHub Actions: separate `rustfmt`, strict `clippy` (`-D warnings`), and multi-OS `cargo test`; release workflows install the toolchain from `rust-toolchain.toml` via `dtolnay/rust-toolchain@v1`.
- Windows x86_64 release **`tkr-x86_64-pc-windows-msvc.tar.gz`** (same tarball layout as other targets; `tkr.exe` inside). **`tkr update`** downloads **`tkr-{triple}.tar.gz`** matching the host Rust triple on Windows (same naming scheme as release CI).
- Dependabot updates for Cargo and GitHub Actions.

### Plugin contract v2

A new plugin API (`tkr_api::plugin::Plugin`) replaces the legacy C-ABI line-filter trait for built-in plugins. The v2 contract adds structured lifecycle hooks, a capability system, and vault-backed storage.

**New types:**
- `Plugin` trait with `on_load`, `on_start`, `on_shutdown`, `on_command_begin`, `on_line`, `on_command_end`, `on_request`
- `Manifest` — declarative plugin metadata (name, version, capabilities, CLI subcommands, storage requests)
- `CommandCtx` / `FilterDecision` — typed filter context replacing raw `(line, command, args, index)` parameters
- `Bus` / `InProcBus` — typed request/reply and event fanout between plugins
- `Vault` / `HostVault` — AES-GCM encrypted key-value store with three sensitivity classes (Public, Private, Secret)
- `Host` — per-plugin interface providing bus, vault, kv, fs, and sqlite handles
- `PluginRegistry` — manages plugin lifecycle (register, load_all, start_all, shutdown_all) and the filter pipeline

**Compatibility:** The legacy `LegacyPlugin` trait (C-ABI `FilterResult`) is still re-exported from `tkr-api` for external cdylib plugins. Internal built-in plugins have migrated to v2.

`PluginRegistry::register` installs a `(plugin, "cli.invoke")` bus handler when the manifest declares non-empty `cli_subcommands`, forwarding to `Plugin::on_request`.

### Encrypted vault

All plugin storage is now routed through an encrypted vault (`~/.tkr/vault/`):

- **Encryption:** age stream cipher (XChaCha20-Poly1305) keyed from a per-installation master key.
- **Master key storage:** `~/.tkr/vault/.tkr-vault.key` (0600). Older releases used the OS keychain; on upgrade, a legacy keychain item is copied to that file once. If the master key cannot be created or read (e.g. some headless CI), the host falls back to an in-memory vault (no persistence).
- **Seal states:** Auto-unsealed (Public class available at boot) and Fully-unsealed (Private + Secret classes available after `tkr vault unseal`).
- **Audit log:** append-only HMAC-chained JSON log of Secret-class reads and writes.

### New `tkr vault` subcommands

```
tkr vault status          # Print current seal state
tkr vault init            # Create vault and write master key to ~/.tkr/vault/.tkr-vault.key
tkr vault unseal          # Promote to fully-unsealed (Private + Secret accessible)
tkr vault seal            # Re-seal the vault
tkr vault rotate          # Re-encrypt all entries under a new master key
tkr vault export [path]   # Export vault as a .tar.gz bundle
tkr vault import <bundle> # Import a vault bundle
tkr vault audit           # Print recent audit log entries
tkr vault audit --verify  # Verify audit log HMAC chain integrity
```

### New `tkr admin` subcommands

```
tkr admin reset --plugin <name>  # Delete all vault entries owned by a plugin
```

### Analytics migration

On first run after upgrading, `AnalyticsPluginV2` automatically migrates existing rows from `~/.tkr/analytics.db` into the encrypted vault sqlite, then renames the legacy file to `~/.tkr/analytics.db.migrated`. **`tkr gain` and `tkr suggest` read only from vault-backed analytics** via `total_savings_via_host`; the legacy file is never queried after migration completes.

### Hooks

`tkr hook universal` accepts either Claude Code's `tool_input.command` JSON shape or a top-level `"command"` string (same hook response shape as `tkr hook claude`).

### `tkr vault` / `tkr admin` in `--help`

`vault` and `admin` are normal clap subcommands (listed in `tkr --help`). `tkr vault` with no subcommand still defaults to **status**.

### Examples

[`examples/README.md`](examples/README.md) and [`examples/manifest.sample.json`](examples/manifest.sample.json) sketch plugin manifest JSON for contributors.

### Analytics tests

Legacy migration tests without the **`test-host`** feature use stub **`Bus`** / **`Vault`** implementations instead of **`unimplemented!`**.

### Plugin contract spec

The full design specification lives at:
`docs/superpowers/specs/2026-04-28-tkr-plugin-contract-v2-design.md`
(gitignored; see the merge commit `d95dcc4` for the feature branch history).

## [0.1.0] — 2026-04-28

Initial public release.

- Token-optimized CLI proxy for 60–90% savings on common dev operations
- Rule-based filter engine (`tkr-filter`) with TOML filter files
- Semantic deduplication via Ollama or exact-match fallback (`tkr-semantic`)
- `tkr gain` / `tkr watch` — token savings analytics and live dashboard
- `tkr discover` — missed-savings analysis from Claude Code history
- Claude Code PreToolUse Bash hook (`tkr hook claude`)
- Homebrew tap: `brew install einyx/tkr/tkr`

> **Historical note:** The standalone **`tkr-semantic`** pipeline was dropped when plugin v2 landed; embeddings for **`tkr suggest`** moved behind **`--features embeddings`**. The bullet above reflects launch-time behavior only.
