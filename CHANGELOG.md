# Changelog

All notable changes to tkr are documented here.

## [Unreleased]

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

### Encrypted vault

All plugin storage is now routed through an encrypted vault (`~/.tkr/vault/`):

- **Encryption:** age stream cipher (XChaCha20-Poly1305) keyed from a per-installation master key.
- **Master key storage:** OS keychain (macOS Keychain / Linux Secret Service) via the `keyring` crate. Falls back to in-memory storage in headless/CI environments (no persistence, but the binary still runs).
- **Seal states:** Auto-unsealed (Public class available at boot) and Fully-unsealed (Private + Secret classes available after `tkr vault unseal`).
- **Audit log:** append-only HMAC-chained JSON log of Secret-class reads and writes.

### New `tkr vault` subcommands

```
tkr vault status          # Print current seal state
tkr vault init            # Create vault and store master key in OS keychain
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

On first run after upgrading, `AnalyticsPluginV2` automatically migrates existing rows from `~/.tkr/analytics.db` into the encrypted vault sqlite, then renames the legacy file to `~/.tkr/analytics.db.migrated`. The `tkr gain` and `tkr suggest` commands continue to read from the legacy database until that path is fully pivoted.

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
