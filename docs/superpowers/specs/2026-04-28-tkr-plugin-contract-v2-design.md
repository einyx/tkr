# tkr Plugin Contract v2 — Design

**Date:** 2026-04-28
**Status:** Approved (brainstorm); pending implementation plan
**Scope:** Foundational. Three follow-on capability tracks (`tkr-burn`, `tkr-mesh`, `tkr-recall`) build on this contract and have their own specs.

## Goal

Extend `tkr` from a stdout-filter host into a plugin host that can also carry long-running services, persistent state, and CLI extensions, behind a stable contract that survives a future move to WASM without source changes for plugin authors.

The `tkr` binary stays single. Capabilities are added by writing plugins, not by forking the binary.

## Non-goals

- WASM plugin loader (deferred; the contract is shaped to accept it).
- The three capability tracks themselves (each is its own spec).
- A plugin marketplace, signing, or distribution story.
- Hot reload of plugins.

## Architecture

Three layers:

1. **`tkr` (host).** Process entry. Owns config loader, plugin loader, capability gate, message bus, vault, and CLI dispatcher. The host is the only component that touches the filesystem outside of plugin sandboxes, opens network sockets on its own behalf, or spawns subprocesses on a plugin's behalf.

2. **`tkr-api` (contract crate).** Defines the `Plugin` trait, lifecycle hooks, the bus message envelope (`Request` / `Reply` / `Event`), host-handle traits (`Host`, `Kv`, `Sqlite`, `Fs`, `Bus`, `Cli`, `Vault`), capability strings, manifest types, and config schema types. Zero host implementation. Only crate plugin authors depend on.

3. **Plugins.** Rust crates implementing `Plugin`. Statically linked into `tkr` for v1.

## Plugin loading model

**Static linkage now, WASM later.** Plugins are Rust crates compiled into `tkr`. The trait surface is intentionally WASM-portable:

- No `&dyn Trait` or lifetime-bearing references in cross-plugin calls.
- All inter-plugin payloads are serde-serializable.
- Host handles are accessed through narrow traits, not raw types (`rusqlite::Connection`, `std::fs::File`, etc. never appear in `tkr-api`).

A future spec adds a WASM loader using the same `Plugin` trait without changing plugin source. Pure dynamic-library loading over Rust ABI is rejected: ABI pain without sandboxing benefit.

## The `Plugin` trait

Sketch (final names settle in the implementation plan):

```rust
pub trait Plugin: Send {
    fn manifest(&self) -> Manifest;

    fn on_load(&mut self, host: &dyn Host) -> Result<()>;
    fn on_start(&mut self) -> Result<()> { Ok(()) }

    // Filter-shaped hooks, opt-in via Manifest.
    fn on_command_begin(&mut self, ctx: &CommandCtx) -> Result<()> { Ok(()) }
    fn on_line(&mut self, line: &str, ctx: &CommandCtx) -> Result<FilterResult> {
        Ok(FilterResult::Pass)
    }
    fn on_command_end(&mut self, ctx: &CommandCtx) -> Result<String> {
        Ok(String::new())
    }

    // Bus-shaped hooks, opt-in via Manifest.
    fn on_event(&mut self, evt: Event) -> Result<()> { Ok(()) }
    fn on_request(&mut self, req: Request) -> Result<Reply> {
        Err(Error::UnknownMethod)
    }

    fn on_shutdown(&mut self) -> Result<()> { Ok(()) }
}
```

`Request`, `Reply`, and `Event` carry method/topic strings and serde-serialized payloads. They never carry references — safe across any boundary.

## Manifest

Each plugin returns a `Manifest`:

- `name`, `version`
- `capabilities_required: Vec<String>`
- `services_exposed: Vec<ServiceSpec>` — method names, request/reply schema
- `events_subscribed: Vec<String>`
- `events_emitted: Vec<String>`
- `cli_subcommands: Vec<CliSpec>` — clap-shaped specs
- `storage_requests: Vec<StorageRequest>` — class + handle kind (kv/sqlite/fs)
- `config_schema: serde_json::Value`

The host validates manifests against capability grants at load. A plugin requesting an ungranted capability fails to load.

## Bus

The host owns one bus. Two operations:

- `host.bus().request(target, method, payload) -> Reply` — typed request/reply RPC. Reply delivered by calling `on_request` on the target plugin.
- `host.bus().emit(topic, payload)` — fire-and-forget. Delivered to every plugin subscribing to `topic` via `on_event`. No reply, no ordering guarantee beyond per-plugin FIFO.

Both go through the capability gate. The bus is in-process and synchronous in v1; the trait shape does not assume sync, so a later spec can swap to async without changing plugin source.

## Storage — vault-centric

Plugin data can be sensitive (mesh signing keys, agent memory, session-log derivatives, peer identities). Storage is therefore **vault-centric**: the host owns a single internal vault, modeled on the seal/unseal idea from HashiCorp Vault, and every plugin storage handle is a namespaced view into it. Plugins do not open paths and do not see plaintext on disk.

### Vault basics

- Single store at `~/.tkr/vault/`. All plugin bytes encrypted at rest.
- Per-plugin namespaces; one plugin cannot read another's data without a bus call through the capability gate.
- Crypto: **age** (X25519 + ChaCha20-Poly1305). Small, modern, audited primitive set; no custom KDF.
- Master key lives in the **OS keychain** by default (Keychain on macOS, Secret Service on Linux, DPAPI on Windows). `tkr unseal` derives subkeys; `tkr seal` zeroes them from memory.
- Optional passphrase mode (`tkr vault init --passphrase`); cold start then prompts.

### Seal state

The vault has two seal levels:

- **Auto-unseal (default).** On process start, the host fetches the master key from the OS keychain and derives a subkey for `public`-class data only. Filter-only plugins and any `public`-class storage work without user action. `private` and `secret` data remain sealed.
- **Full unseal.** `tkr unseal` derives the `private` and `secret` subkeys. Required for `private`/`secret` reads and writes. Plugins that requested those classes block in `on_start` until the user runs `tkr unseal` (or until a configured auto-unseal hook fires for trusted environments).
- **Sealed.** `tkr seal` zeroes the `private`/`secret` subkeys in memory; auto-unseal subkey persists until process exit. On-disk bytes stay encrypted in all states.

If the user opted into passphrase mode (`tkr vault init --passphrase`), there is no auto-unseal: even `public` data requires `tkr unseal` after process start.

`tkr vault status / seal / unseal / rotate` are host-owned subcommands.

### Sensitivity classes

Plugins declare a class per storage request:

- `public` — encrypted at rest, listable across plugins via the bus (with capability), low-friction. Auto-unseal allowed.
- `private` — encrypted at rest, not listable across plugins, only the owning plugin can read.
- `secret` — encrypted at rest, access logged to the audit log, only readable while unsealed, never returned to other plugins even via the bus.

The host enforces the class; plugins cannot upgrade their own data without re-declaring it.

### Storage handles

Every read/write goes through the vault:

- `host.kv(class) -> &dyn Kv` — string-keyed serde value store.
- `host.sqlite(schema_sql, class) -> &dyn Sqlite` — sqlite database in the plugin's namespace, transparently encrypted. Exact mechanism (SQLCipher-style page encryption vs sealed snapshot vs per-row) deferred to the implementation plan.
- `host.fs(class) -> &dyn Fs` — read/write/list within the plugin's vault namespace only.

Handles do not expose `rusqlite::Connection`, `std::fs::File`, or any type that lets a plugin escape the vault.

### Capabilities (storage subset)

Capability strings replace the looser `cap:fs.*` set:

- `cap:vault.read.public`, `cap:vault.write.public`
- `cap:vault.read.private`, `cap:vault.write.private`
- `cap:vault.read.secret`, `cap:vault.write.secret`
- `cap:vault.unseal` — only the host CLI holds this by default
- `cap:vault.audit.read` — for an admin/inspection plugin

A plugin requesting a class above its grants fails to load.

### Audit log

Every `secret`-class read and write is appended to a tamper-evident log (hash-chained entries) inside the vault. `tkr vault audit` surfaces it. Host-only; plugins can write but not edit or truncate.

### Backup, migration, wipe

Host concerns:

- `tkr vault export` → sealed bundle (still encrypted, restore requires master key).
- `tkr vault import` → restore from bundle.
- `tkr vault rotate` → re-encrypts under a new master key without plaintext intermediates.
- `tkr admin reset --plugin <name>` removes a plugin's namespace inside the vault.

Plugins do not implement their own backup.

## Config

Single source of truth: `~/.tkr/config.toml`, with `[plugin.<name>]` sections, validated against each plugin's declared JSON schema at `on_load` and handed back as a typed struct. Validation failures abort startup with the offending plugin and field named.

Existing `filters/*.toml` continue working as a per-plugin overlay (lower priority than `config.toml`) for backwards compatibility.

Sensitive config values (API tokens, etc.) live **in the vault**, not in `config.toml`. Plugins request them via `host.vault().read_secret("plugin/<name>/<key>")` after declaring the appropriate `cap:vault.read.secret` capability. `tkr vault put <plugin>/<key>` is the user-facing way to seed them.

## Capabilities (full set)

Beyond the vault subset above:

- `cap:stdout.filter` — register filter-shaped hooks
- `cap:cli.subcommand` — register `tkr <plugin> <subcmd>`
- `cap:net.outbound` — open outbound sockets via host-issued client handles
- `cap:subprocess` — spawn subprocesses via the host
- `cap:bus.call.<method-pattern>` — invoke specific bus methods exposed by other plugins

Manifest declares required; `~/.tkr/config.toml` grants. Enforced at bus calls, vault opens, CLI registration, subprocess spawn, and (future) network. Trivially bypassable under static linkage; load-bearing once the WASM loader ships.

## CLI extension

Plugins declare clap-shaped subcommand specs in their manifest. Host mounts under a fixed namespace:

```
tkr <plugin-name> <subcmd> [args...]
```

Collisions impossible. Host dispatches an invocation by emitting a `cli.invoke` request to the owning plugin; the plugin handles it through `on_request` and returns an exit code + stdout/stderr in the reply.

Existing top-level subcommands (`tkr gain`, `tkr proxy`, `tkr discover`, `tkr vault …`, `tkr seal`, `tkr unseal`) remain host-owned and live outside the plugin namespace.

## Migration of existing code

- **`tkr-api`** — extend the `Plugin` trait with new hooks. All new hooks have default impls so existing filter plugins keep compiling unchanged.
- **`tkr-filter`** — no behavior change. Plugins gain a `cap:stdout.filter` declaration and migrate from `filter()` to `on_line()` (old name kept as deprecated re-export for one release).
- **`tkr-analytics`** — keep current sqlite, but reach storage through `host.sqlite(class=Public)`. Existing `~/.tkr/analytics.db` is migrated into the vault on first run under the new binary.
- **New plugin crates** (`tkr-burn`, `tkr-mesh`, `tkr-recall`) land in later specs against this contract.

## Error handling

- **Load-time errors** (manifest invalid, capability ungranted, config schema mismatch, vault sealed for required class): abort startup, name the offending plugin, exit non-zero.
- **Start-time errors** (`on_start` returns `Err`): plugin marked **degraded**, log, continue. `tkr status` lists degraded plugins.
- **Bus errors** (unknown method, capability denied, payload deserialization fails): typed `Error` variants in `Result`. Host does not panic.
- **Filter-path errors** (`on_line` returns `Err`): line passes through unchanged; plugin marked degraded for the rest of the command run.
- **Vault errors** (sealed, key denied, audit write fails): typed; reads return `Err(Sealed)` to plugins that requested unsealed-only classes.
- **Shutdown errors**: logged, never block exit.

## Testing

- **`tkr-api` ships `test-host`**: in-memory implementations of `Host`, `Bus`, `Kv`, `Sqlite`, `Fs`, `Vault`. Tempdir-backed; capability gate configurable; vault uses an ephemeral master key. Plugins unit-test against the same contract.
- **Contract conformance tests** in `tkr-api` exercise every host-facing trait method against `test-host`.
- **Integration tests** in `tkr` run the real host against fixture plugins covering each lifecycle path (load failure, start failure, bus call, capability denial, CLI dispatch, sealed-vault block, shutdown).
- **Vault-specific tests**: seal/unseal cycle, audit log integrity (corruption detected), rotate end-to-end, sandbox escape attempts (must fail).

## Open questions deferred to the implementation plan

- Exact name of the `Plugin` trait's successor (`Plugin`, `TkrPlugin`, `Component`).
- Whether `Manifest` is returned from `on_load` or a separate `manifest()` method (sketched both ways).
- `host.sqlite()` API shape: query builder vs raw prepared statements with serde row mapping.
- SQLite encryption mechanism: SQLCipher-style page encryption, sealed snapshot blob, or per-row.
- `cli.invoke` reply shape: structured (`exit_code`, `stdout`, `stderr`) vs streaming.
- Audit log retention and rotation policy.

## Follow-on specs (not part of this one)

- **`tkr-burn`** — passive disk-side attribution of AI coding token spend by task / tool / model / MCP / project, plugged into `tkr gain`.
- **`tkr-mesh`** — signed agent-to-agent push channel between sessions, surfaced via MCP. Mesh signing keys live in the vault as `secret`-class.
- **`tkr-recall`** — durable, branchable, rollback-able memory store for agents with semantic + literal retrieval. Memory bodies live in the vault as `private`-class by default, `secret`-class on opt-in.

Each is independently shippable on top of this contract.
