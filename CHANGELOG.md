# Changelog

All notable changes to tkr are documented here.

## [Unreleased] — security hardening

Pre-deployment security review fixes. Issues identified in a four-agent
deep review of the new mesh / broker / contract / MCP surfaces.

### Critical

- **tkr-server**: `POST /api/v1/mesh/join` and `GET /api/v1/mesh/ws` now
  require an authenticated session — previously any internet caller with
  a valid invite payload could enroll arbitrary addresses and connect
  to the broker.
- **tkr-mesh**: `JoinedMesh` no longer derives `Debug`/`Serialize`/
  `Deserialize`. The on-chain signing key is redacted from `Debug` output;
  callers persist via new `JoinedMesh::save()` / `JoinedMesh::load()`,
  which writes the file mode 0o600 atomically on Unix.
- **tkr-mcp**: `tkr_outline_file`, `tkr_find_symbol`, `tkr_grep_summary`
  now confine all caller-supplied paths under a project root (defaults to
  CWD; override with `TKR_MCP_ROOT`). Previously they could read any file
  on the filesystem.

### High

- **tkr-server**: CORS no longer reflects arbitrary origins with
  credentials. Origins are matched against an allowlist
  (`localhost:3001/4000`, `tkr.prysm.sh`, `TKR_ALLOWED_ORIGIN`).
- **tkr-server**: session cookie now sets `Secure`. Session ID is now a
  256-bit `OsRng` value (was timestamp-derived).
- **tkr-server**: `read_json` caps request bodies at 4 MiB
  (`TKR_MAX_BODY_BYTES` to override) — prevents memory-exhaustion DoS.
- **MeshEscrow.sol**: `claim()` is now restricted to `ch.recipient`
  (otherwise a third party with a copy of a valid receipt could grief a
  recipient contract that reverts on receive). `close()` is restricted
  to `ch.payer` (otherwise an MEV bot could front-run a recipient's
  expiry-window claim).
- **tkr (vault)**: `vault rotate` no longer prints the new master key
  to stderr on persist failure — the key would otherwise land in shell
  scrollback / parent-process logs.

### Tests

- `tkr-server` mesh E2E test now logs in first and threads the session
  cookie through join + WS upgrade. New `TKR_MESH_WS_COOKIE` env var on
  `tkr_mesh::Client::connect` for authenticated brokers.
- `MeshEscrow.t.sol`: +3 tests (`test_claim_only_recipient_can_call`,
  `test_close_revert_not_payer`, `test_claim_revert_not_recipient`).
  20/20 passing.

### Defence in depth (follow-up)

- **tkr-mesh**: invite EIP-712 domain now matches the standard four-field
  shape (`name, version, chainId, verifyingContract`) — `chainId = 0`,
  `verifyingContract = address(0)` since invites are off-chain. Brings
  invites in line with the on-chain `MeshEscrow` receipt domain and
  removes any structural ambiguity if a future on-chain invite registry
  is added. **Breaking**: invalidates any previously-issued invite
  signatures (none in production yet).
- **tkr-mesh**: `Hello.session_id` is now a 128-bit `OsRng` nonce (was
  `pid + now_ms`). New `Hello::verify_with_now(now_ms, max_skew_ms)` and
  `HELLO_MAX_SKEW_MS = 60s`; the broker calls the freshness-checking
  variant so a captured Hello frame can't be replayed by a network
  adversary later.
- **tkr-sandbox**: `SandboxPolicy.env_allow` opt-in allowlist; child
  processes now spawn with `env_clear()` + `PATH` only by default.
  Prevents leaking `ANTHROPIC_API_KEY`, `AWS_*`, `GITHUB_TOKEN`, etc.
  into sandboxed code that has network access. Applies to both linux
  (Landlock) and macos (`sandbox-exec`) backends.
- **tkr-session-recorder**: new `scrub` module. Commands like `cat`,
  `env`, `printenv`, `op`, `gpg` (full deny list in `scrub.rs`) suppress
  the `output_preview` entirely; for everything else, lines matching
  common API-key, bearer-token, AWS access-key, or PEM-block patterns
  are replaced with `<redacted: …>` before being persisted to the vault.

## [0.3.0] — 2026-05-02

Major: tkr is no longer just a Bash filter. New surfaces extend it into
peer-messaging, on-chain payments, and structured code-intelligence over MCP.

### Agent mesh (`tkr-mesh` + `tkr mesh` CLI)

- Peer messaging across machines via a public broker
  (`wss://tkr.prysm.sh/api/v1/mesh/ws`).
- Identity = secp256k1 keypair (same shape as an Ethereum wallet); the
  on-mesh address is the EIP-55 Ethereum address.
- E2E-encrypted DMs (ECDH + AES-256-GCM); the broker only sees ciphertext.
- EIP-712 invites — wallet-renderable, signature-verified.
- Five-command CLI: `tkr mesh invite-mint / join / list / whoami / tail / send`.
- New broker in `tkr-server`: `POST /api/v1/mesh/join`, `GET /api/v1/mesh/ws`,
  `GET /api/v1/mesh/status` (live peer counts on the dashboard).

### On-chain payments (`tkr pay` + `MeshEscrow.sol`)

- `MeshEscrow.sol` — payment-channel contract on Base. Open with a deposit,
  recipient claims with EIP-712 receipts, payer reclaims unspent funds after
  a deadline. 17/17 forge tests passing.
- `tkr pay receipt-issue` — sign a receipt off-chain.
- `tkr pay receipt-verify` — verify a receipt locally.
- `tkr pay claim` — submit on-chain via alloy (rustls-only). Verified against
  an anvil-fork of Base mainnet end-to-end.
- `make demo-payment` — full receipt flow on local anvil in ~10 s.

### MCP server (`tkr-mcp` + `tkr mcp`)

- Stdio JSON-RPC 2.0 server registered under `mcpServers.tkr` in
  `~/.claude/settings.json`.
- Three tools that return structured summaries instead of raw text:
  - `tkr_outline_file` — symbol kind/name/range for Rust, Python, Go,
    TypeScript, JavaScript (~75-95% token reduction on real source files).
  - `tkr_find_symbol` — definitions of a symbol across the tree, .gitignore-
    aware.
  - `tkr_grep_summary` — regex search grouped by file with per-file caps.
- `~/.claude/tkr.md` fragment installed alongside, included from CLAUDE.md
  via `@tkr.md`. Steers the model to prefer tkr_* tools for large
  Reads / broad Greps.

### PostToolUse hook

- New `tkr hook post` for Claude Code's `PostToolUse` event (Read|Grep|Glob).
  Adds a steering note via `additionalContext` when a tool result was
  likely-too-large. Cannot rewrite results that already entered context —
  that's what the MCP path is for.
- `tkr install --claude` now wires three things at once: the existing
  Bash PreToolUse hook, the new PostToolUse hook, and the MCP server
  registration. Idempotent on re-install.

### React dashboard

- New `crates/tkr-server/web/` — React + TypeScript + Vite. Vite's
  `viteSingleFile` plugin emits a single inlined HTML to
  `crates/tkr-server/static/index.html`, kept embedded via `include_str!`.
- 4 views: Landing (public, live mesh stats), Login, Dashboard
  (Mesh + Sessions panels), Session detail.
- Dockerfile is now 3-stage: `node:20-slim` builds the bundle,
  `rust:1.88-slim` builds the binary, `debian:bookworm-slim` runs.

### Filter additions

- New rule types: `truncate_long`, `context_window`, `dedup_with_count`,
  `empty_result_substitute`, `group_by_capture`, `substitute_words`,
  plus a `flush_summary` aggregation channel.
- New filters: `find.toml`, `grep.toml`. Major rewrites of `git.toml`
  (porcelain `M/A/D/R` short codes — 95% reduction on `git status`),
  `ls.toml` (drops noisy dirs), `npm.toml` (restored success summary +
  added empty-result marker), `docker.toml` (log dedup + level filter).

### Server hardening

- `TKR_ADMIN_PASSWORD` env var (≥ 8 chars) now required for non-loopback
  bind. Loopback retains a dev fallback with a stderr warning. Login
  uses constant-time comparison.
- Default `HOST` flipped from `0.0.0.0` to `127.0.0.1` (nginx-on-same-host
  is the expected deploy shape).
- `docker-compose.yml` shipped with the recommended deployment, plus a
  hardened `systemd` unit alternative.

### Operator tools

- `tkr install --with-foundry` — installs the foundry toolchain
  alongside the AI-tool hook.
- `deploy/keys/` scaffold for per-network throwaway deploy keys
  (Sepolia, mainnet, …) with chmod-0600 enforcement and a README.

### Security review

A focused security review of the 33-commit branch landed clean:
no exploitable vulnerabilities at confidence ≥ 8. Notable sub-threshold
items (Hello-replay → routing hijack mitigated by E2E ECIES; idempotent
pre-enrollment; broker-host SSRF excluded by rules) all documented.

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
