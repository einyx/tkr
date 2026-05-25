# Changelog

All notable changes to jkr are documented here.

## [Unreleased] — AI gateway

Tkr-server grew from "Anthropic proxy + mesh dashboard" into a full
AI gateway. New surfaces share the same Logto-SSO identity, the same
`SseUsageAccumulator` for usage extraction, the same `RedactionEngine`
for pre-flight + response-side credential scrubbing, and the same
`jkr-sandbox` crate the CLI agents already used. See `docs/operations.md`
and `docs/integration.md` for end-user docs.

### Proxy

- **`/v1/messages`** (Anthropic-wire) — passthrough proxy with TLS
  upstream (`ureq`+rustls), streaming SSE, header preservation,
  receipt + usage extraction.
- **`/v1/chat/completions`** (OpenAI-wire) — same shape, OpenAI auth
  headers.
- **`ProviderProxy` consts** collapse both handler shapes into one
  shared `proxy_llm_request` + `proxy_llm_streaming`. Adding a third
  provider is now a few lines.
- **Concurrency cap** (`JKR_UPSTREAM_MAX_CONCURRENT`, default 64) —
  semaphore-bounded; over-cap returns 429 + `Retry-After: 1`.

### Filter

- **Pre-flight redaction** (`RedactionEngine`) — AWS keys, GitHub PATs
  (classic + fine-grained), OpenAI / Anthropic keys, Slack tokens,
  JWTs. Counters on `/api/v1/filter/stats`.
- **Response-side redaction** — non-streaming + streaming (`SseRewriter`
  buffers events, JSON-parses `data:` lines, scrubs known delta paths,
  re-serialises).
- **Prompt-injection heuristic** (`InjectionEngine`) — log-by-default
  rules: `ignore-previous`, `disregard-above`, `dan-jailbreak`,
  `system-role-inject`, `assistant-role-inject`. Opt-in `Block` returns
  400.

### Receipts

- **Signed audit receipts** — every LLM call gets a secp256k1 ECDSA
  signature over a canonical message
  (`v1\nts=…\nprovider=…\n…`). Key persists at
  `JKR_RECEIPT_SIGNING_KEY_PATH`. Public-landing `VerifyReceipt`
  tool reconstructs canonical bytes for offline verification.
- **Audit drain queue** — FIFO with cap + drop counter (loud signal
  when the relayer falls behind). `POST /api/v1/llm/receipts/drain`
  + dashboard "drain now" button.
- **Optional body capture** (`JKR_CAPTURE_BODIES=true`, off-default) —
  rolling ring of scrubbed bodies, dashboard `CapturedPanel` with
  download-as-JSONL.

### Sandbox

- **`POST /api/v1/sandbox/exec`** (`JKR_SANDBOX_EXEC=true`, off-default,
  Logto-auth-gated) — wraps `jkr_sandbox::run_sandboxed`. Hardcoded
  binary allowlist; per-request policy (no network, empty env,
  read-only loader paths). `/sandbox/stats` + `/sandbox/recent`.

### Identity

- **Logto OIDC code-flow** — `/auth/logto/{start,callback}` mint
  `jkr_session` cookies. Pending PKCE state in Redis or in-memory.
- **Login auto-redirect** — `/login` goes straight to
  `/auth/logto/start`.

### Persistence

- **Postgres + Redis** in docker-compose with volumes. `DATABASE_URL`
  + `REDIS_URL` env vars. Pools init at startup, missing-env loudly
  logged but Option-typed so tests + dev work without.
- **Migrations** (`crates/jkr-server/migrations/`) — schema for
  `sessions`, `receipts_queue`, `llm_recent`, `sandbox_recent`.

### Dashboard + landing

- **Dashboard** rebuilt as the AI gateway control surface — identity
  header, status banner (capture/sandbox/drainer chips), token-usage
  hero with sparkline, filter, receipts, sandbox, captured calls,
  receipt-verify tool, mesh + ingested sessions kept as secondary.
- **Landing** rewritten in Prysm voice — pillar cards
  (01 proxy / 02 filter / 03 sandbox / 04 receipts), thesis block,
  live-gateway stats, public receipt-verify tool, primitives.
- **Component split** — `views/{Dashboard,Landing}.tsx` are thin
  orchestrators; per-panel + per-section components live under
  `components/{dashboard,landing}/`.

### Docs

- **`docs/integration.md`** — IDE-by-IDE setup, security model,
  filter/injection behaviour, edge-ratelimit snippet.
- **`docs/operations.md`** — env-flag matrix, two-sandbox explainer
  (CLI vs server-side HTTP), receipt verification protocol, dashboard
  panel reference.

### Known gaps (queued)

- CLI agent → server sandbox ingest path.
- Persistence layer INSERTs for receipts / sandbox runs (pools wired,
  not yet on the proxy path).
- Persistent signing-key volume mount (defaults to ephemeral
  in-memory key with startup warning).

---

## [Unreleased] — security hardening

Pre-deployment security review fixes. Issues identified in a four-agent
deep review of the new mesh / broker / contract / MCP surfaces.

### Critical

- **jkr-server**: `POST /api/v1/mesh/join` and `GET /api/v1/mesh/ws` now
  require an authenticated session — previously any internet caller with
  a valid invite payload could enroll arbitrary addresses and connect
  to the broker.
- **jkr-mesh**: `JoinedMesh` no longer derives `Debug`/`Serialize`/
  `Deserialize`. The on-chain signing key is redacted from `Debug` output;
  callers persist via new `JoinedMesh::save()` / `JoinedMesh::load()`,
  which writes the file mode 0o600 atomically on Unix.
- **jkr-mcp**: `jkr_outline_file`, `jkr_find_symbol`, `jkr_grep_summary`
  now confine all caller-supplied paths under a project root (defaults to
  CWD; override with `JKR_MCP_ROOT`). Previously they could read any file
  on the filesystem.

### High

- **jkr-server**: CORS no longer reflects arbitrary origins with
  credentials. Origins are matched against an allowlist
  (`localhost:3001/4000`, `tkr.prysm.sh`, `JKR_ALLOWED_ORIGIN`).
- **jkr-server**: session cookie now sets `Secure`. Session ID is now a
  256-bit `OsRng` value (was timestamp-derived).
- **jkr-server**: `read_json` caps request bodies at 4 MiB
  (`JKR_MAX_BODY_BYTES` to override) — prevents memory-exhaustion DoS.
- **MeshEscrow.sol**: `claim()` is now restricted to `ch.recipient`
  (otherwise a third party with a copy of a valid receipt could grief a
  recipient contract that reverts on receive). `close()` is restricted
  to `ch.payer` (otherwise an MEV bot could front-run a recipient's
  expiry-window claim).
- **jkr (vault)**: `vault rotate` no longer prints the new master key
  to stderr on persist failure — the key would otherwise land in shell
  scrollback / parent-process logs.

### Tests

- `jkr-server` mesh E2E test now logs in first and threads the session
  cookie through join + WS upgrade. New `JKR_MESH_WS_COOKIE` env var on
  `jkr_mesh::Client::connect` for authenticated brokers.
- `MeshEscrow.t.sol`: +3 tests (`test_claim_only_recipient_can_call`,
  `test_close_revert_not_payer`, `test_claim_revert_not_recipient`).
  20/20 passing.

### Defence in depth (follow-up)

- **jkr-mesh**: invite EIP-712 domain now matches the standard four-field
  shape (`name, version, chainId, verifyingContract`) — `chainId = 0`,
  `verifyingContract = address(0)` since invites are off-chain. Brings
  invites in line with the on-chain `MeshEscrow` receipt domain and
  removes any structural ambiguity if a future on-chain invite registry
  is added. **Breaking**: invalidates any previously-issued invite
  signatures (none in production yet).
- **jkr-mesh**: `Hello.session_id` is now a 128-bit `OsRng` nonce (was
  `pid + now_ms`). New `Hello::verify_with_now(now_ms, max_skew_ms)` and
  `HELLO_MAX_SKEW_MS = 60s`; the broker calls the freshness-checking
  variant so a captured Hello frame can't be replayed by a network
  adversary later.
- **jkr-sandbox**: `SandboxPolicy.env_allow` opt-in allowlist; child
  processes now spawn with `env_clear()` + `PATH` only by default.
  Prevents leaking `ANTHROPIC_API_KEY`, `AWS_*`, `GITHUB_TOKEN`, etc.
  into sandboxed code that has network access. Applies to both linux
  (Landlock) and macos (`sandbox-exec`) backends.
- **jkr-session-recorder**: new `scrub` module. Commands like `cat`,
  `env`, `printenv`, `op`, `gpg` (full deny list in `scrub.rs`) suppress
  the `output_preview` entirely; for everything else, lines matching
  common API-key, bearer-token, AWS access-key, or PEM-block patterns
  are replaced with `<redacted: …>` before being persisted to the vault.

## [0.3.0] — 2026-05-02

Major: jkr is no longer just a Bash filter. New surfaces extend it into
peer-messaging, on-chain payments, and structured code-intelligence over MCP.

### Agent mesh (`jkr-mesh` + `jkr mesh` CLI)

- Peer messaging across machines via a public broker
  (`wss://tkr.prysm.sh/api/v1/mesh/ws`).
- Identity = secp256k1 keypair (same shape as an Ethereum wallet); the
  on-mesh address is the EIP-55 Ethereum address.
- E2E-encrypted DMs (ECDH + AES-256-GCM); the broker only sees ciphertext.
- EIP-712 invites — wallet-renderable, signature-verified.
- Five-command CLI: `jkr mesh invite-mint / join / list / whoami / tail / send`.
- New broker in `jkr-server`: `POST /api/v1/mesh/join`, `GET /api/v1/mesh/ws`,
  `GET /api/v1/mesh/status` (live peer counts on the dashboard).

### On-chain payments (`jkr pay` + `MeshEscrow.sol`)

- `MeshEscrow.sol` — payment-channel contract on Base. Open with a deposit,
  recipient claims with EIP-712 receipts, payer reclaims unspent funds after
  a deadline. 17/17 forge tests passing.
- `jkr pay receipt-issue` — sign a receipt off-chain.
- `jkr pay receipt-verify` — verify a receipt locally.
- `jkr pay claim` — submit on-chain via alloy (rustls-only). Verified against
  an anvil-fork of Base mainnet end-to-end.
- `make demo-payment` — full receipt flow on local anvil in ~10 s.

### MCP server (`jkr-mcp` + `jkr mcp`)

- Stdio JSON-RPC 2.0 server registered under `mcpServers.jkr` in
  `~/.claude/settings.json`.
- Three tools that return structured summaries instead of raw text:
  - `jkr_outline_file` — symbol kind/name/range for Rust, Python, Go,
    TypeScript, JavaScript (~75-95% token reduction on real source files).
  - `jkr_find_symbol` — definitions of a symbol across the tree, .gitignore-
    aware.
  - `jkr_grep_summary` — regex search grouped by file with per-file caps.
- `~/.claude/jkr.md` fragment installed alongside, included from CLAUDE.md
  via `@jkr.md`. Steers the model to prefer jkr_* tools for large
  Reads / broad Greps.

### PostToolUse hook

- New `jkr hook post` for Claude Code's `PostToolUse` event (Read|Grep|Glob).
  Adds a steering note via `additionalContext` when a tool result was
  likely-too-large. Cannot rewrite results that already entered context —
  that's what the MCP path is for.
- `jkr install --claude` now wires three things at once: the existing
  Bash PreToolUse hook, the new PostToolUse hook, and the MCP server
  registration. Idempotent on re-install.

### React dashboard

- New `crates/jkr-server/web/` — React + TypeScript + Vite. Vite's
  `viteSingleFile` plugin emits a single inlined HTML to
  `crates/jkr-server/static/index.html`, kept embedded via `include_str!`.
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

- `JKR_ADMIN_PASSWORD` env var (≥ 8 chars) now required for non-loopback
  bind. Loopback retains a dev fallback with a stderr warning. Login
  uses constant-time comparison.
- Default `HOST` flipped from `0.0.0.0` to `127.0.0.1` (nginx-on-same-host
  is the expected deploy shape).
- `docker-compose.yml` shipped with the recommended deployment, plus a
  hardened `systemd` unit alternative.

### Operator tools

- `jkr install --with-foundry` — installs the foundry toolchain
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
- Windows x86_64 release **`jkr-x86_64-pc-windows-msvc.tar.gz`** (same tarball layout as other targets; `jkr.exe` inside). **`jkr update`** downloads **`jkr-{triple}.tar.gz`** matching the host Rust triple on Windows (same naming scheme as release CI).
- Dependabot updates for Cargo and GitHub Actions.

### Plugin contract v2

A new plugin API (`jkr_api::plugin::Plugin`) replaces the legacy C-ABI line-filter trait for built-in plugins. The v2 contract adds structured lifecycle hooks, a capability system, and vault-backed storage.

**New types:**
- `Plugin` trait with `on_load`, `on_start`, `on_shutdown`, `on_command_begin`, `on_line`, `on_command_end`, `on_request`
- `Manifest` — declarative plugin metadata (name, version, capabilities, CLI subcommands, storage requests)
- `CommandCtx` / `FilterDecision` — typed filter context replacing raw `(line, command, args, index)` parameters
- `Bus` / `InProcBus` — typed request/reply and event fanout between plugins
- `Vault` / `HostVault` — AES-GCM encrypted key-value store with three sensitivity classes (Public, Private, Secret)
- `Host` — per-plugin interface providing bus, vault, kv, fs, and sqlite handles
- `PluginRegistry` — manages plugin lifecycle (register, load_all, start_all, shutdown_all) and the filter pipeline

**Compatibility:** The legacy `LegacyPlugin` trait (C-ABI `FilterResult`) is still re-exported from `jkr-api` for external cdylib plugins. Internal built-in plugins have migrated to v2.

`PluginRegistry::register` installs a `(plugin, "cli.invoke")` bus handler when the manifest declares non-empty `cli_subcommands`, forwarding to `Plugin::on_request`.

### Encrypted vault

All plugin storage is now routed through an encrypted vault (`~/.jkr/vault/`):

- **Encryption:** age stream cipher (XChaCha20-Poly1305) keyed from a per-installation master key.
- **Master key storage:** `~/.jkr/vault/.jkr-vault.key` (0600). Older releases used the OS keychain; on upgrade, a legacy keychain item is copied to that file once. If the master key cannot be created or read (e.g. some headless CI), the host falls back to an in-memory vault (no persistence).
- **Seal states:** Auto-unsealed (Public class available at boot) and Fully-unsealed (Private + Secret classes available after `jkr vault unseal`).
- **Audit log:** append-only HMAC-chained JSON log of Secret-class reads and writes.

### New `jkr vault` subcommands

```
jkr vault status          # Print current seal state
jkr vault init            # Create vault and write master key to ~/.jkr/vault/.jkr-vault.key
jkr vault unseal          # Promote to fully-unsealed (Private + Secret accessible)
jkr vault seal            # Re-seal the vault
jkr vault rotate          # Re-encrypt all entries under a new master key
jkr vault export [path]   # Export vault as a .tar.gz bundle
jkr vault import <bundle> # Import a vault bundle
jkr vault audit           # Print recent audit log entries
jkr vault audit --verify  # Verify audit log HMAC chain integrity
```

### New `jkr admin` subcommands

```
jkr admin reset --plugin <name>  # Delete all vault entries owned by a plugin
```

### Analytics migration

On first run after upgrading, `AnalyticsPluginV2` automatically migrates existing rows from `~/.jkr/analytics.db` into the encrypted vault sqlite, then renames the legacy file to `~/.jkr/analytics.db.migrated`. **`jkr gain` and `jkr suggest` read only from vault-backed analytics** via `total_savings_via_host`; the legacy file is never queried after migration completes.

### Hooks

`jkr hook universal` accepts either Claude Code's `tool_input.command` JSON shape or a top-level `"command"` string (same hook response shape as `jkr hook claude`).

### `jkr vault` / `jkr admin` in `--help`

`vault` and `admin` are normal clap subcommands (listed in `jkr --help`). `jkr vault` with no subcommand still defaults to **status**.

### Examples

[`examples/README.md`](examples/README.md) and [`examples/manifest.sample.json`](examples/manifest.sample.json) sketch plugin manifest JSON for contributors.

### Analytics tests

Legacy migration tests without the **`test-host`** feature use stub **`Bus`** / **`Vault`** implementations instead of **`unimplemented!`**.

### Plugin contract spec

The full design specification lives at:
`docs/superpowers/specs/2026-04-28-jkr-plugin-contract-v2-design.md`
(gitignored; see the merge commit `d95dcc4` for the feature branch history).

## [0.1.0] — 2026-04-28

Initial public release.

- Token-optimized CLI proxy for 60–90% savings on common dev operations
- Rule-based filter engine (`jkr-filter`) with TOML filter files
- Semantic deduplication via Ollama or exact-match fallback (`jkr-semantic`)
- `jkr gain` / `jkr watch` — token savings analytics and live dashboard
- `jkr discover` — missed-savings analysis from Claude Code history
- Claude Code PreToolUse Bash hook (`jkr hook claude`)
- Homebrew tap: `brew install einyx/jkr/jkr`

> **Historical note:** The standalone **`jkr-semantic`** pipeline was dropped when plugin v2 landed; embeddings for **`jkr suggest`** moved behind **`--features embeddings`**. The bullet above reflects launch-time behavior only.
