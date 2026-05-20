# Operating tkr-server

Quick reference for running tkr in production: env flags, the two
sandboxes (CLI agent-side vs server-side HTTP), how to verify a
receipt signature offline, what each dashboard panel actually shows.

If you're integrating a client, start at [`integration.md`](integration.md).

---

## Env-var cheat sheet

Set on the `tkr-server` container (via `tkr-server.env` in `docker-compose.yml`).

| Var | What it enables | Default |
|---|---|---|
| `TKR_ANTHROPIC_UPSTREAM` | `/v1/messages` proxy → this URL | unset (503) |
| `TKR_OPENAI_UPSTREAM` | `/v1/chat/completions` proxy → this URL | unset (503) |
| `TKR_LOGTO_{ENDPOINT,APP_ID,APP_SECRET,REDIRECT_URI}` | Logto SSO | unset (sign-in unavailable) |
| `TKR_UPSTREAM_MAX_CONCURRENT` | in-process upstream concurrency cap | `64` |
| `TKR_CAPTURE_BODIES` | stash scrubbed request + response bodies in a ring | `false` |
| `TKR_SANDBOX_EXEC` | expose `POST /api/v1/sandbox/exec` | `false` |
| `TKR_RECEIPT_SIGNING_KEY_PATH` | where the secp256k1 signing key lives on disk | `/var/lib/tkr/receipt-signing-key` |
| `DATABASE_URL` | Postgres for durable state | unset (in-memory fallback) |
| `REDIS_URL` | Redis for hot ephemeral state (OAuth pending, future rate-limit) | unset (in-memory fallback) |

**Defaults are deliberately conservative.** "Off" for capture/sandbox/persistence means "you must opt in." The public-landing claim "your prompts never leave" only holds with the defaults; turning on capture changes the trust story.

---

## The two sandboxes

There are two callers of the `tkr-sandbox` crate. Same code, different surfaces, different telemetry. They are easy to confuse — they have the same security primitive (Landlock on Linux, sandbox-exec on macOS) but talk through different channels.

### CLI agent-side (`tkr <cmd>` on a host)

- Runs entirely on the host where the CLI executes.
- Wraps any command (`tkr cargo test`, `tkr npm install`, `tkr bash`) in a per-session jail.
- **No network calls back to tkr-server.** Lives and dies on the host.
- Pre-dates the HTTP endpoint by months — the dashboard's three pillar bullets ("filesystem · network · lifetime") describe THIS sandbox.

### Server-side HTTP (`POST /api/v1/sandbox/exec`)

- Endpoint on tkr-server itself, auth-gated to Logto sessions.
- Allowlisted binaries only (`cat, ls, echo, head, tail, wc, grep, find, sort, uniq, pwd, date, true, false, env`).
- Returns `{exit, stdout, stderr, truncated, duration_ms}`.
- **This is what the dashboard's `Sandbox` panel shows.** The "test sandbox" button hits this endpoint. CLI activity is NOT visible here today.

### "Why don't my CLI runs show up in the dashboard?"

Because the CLI doesn't yet POST sandbox events back to tkr-server. There is no telemetry ingest path. Adding one is a separate slice (new server route + CLI-side reporter); until then, agent activity is local-only.

---

## Receipt signature protocol

Every receipt the server emits is signed with secp256k1 ECDSA. To verify offline:

### Canonical message (signed bytes)

```
v1\nts={ts}\nprovider={provider}\nmodel={model}\nstatus={status}\ninput_tokens={input_tokens}\noutput_tokens={output_tokens}\nduration_ms={duration_ms}
```

Newline-separated, `v1\n` prefix, no trailing newline. Field order is locked by the `signer_canonical_message_is_stable` test in `crates/tkr-server/src/main.rs`.

### Verifying

1. Take a receipt JSON from `GET /api/v1/llm/recent` or `POST /api/v1/llm/receipts/drain`.
2. Reconstruct the canonical message from the receipt's fields.
3. Decode `signature` (hex, strip `0x`) — 64-byte compact ECDSA.
4. Decode `signer_pubkey` (hex, strip `0x`) — 33-byte compressed secp256k1 pubkey.
5. Verify signature against the message bytes using k256 / any ECDSA library that speaks compact-form sigs + compressed sec1 pubkeys.

Mismatched signature = receipt was forged or the canonical message format drifted. The `sig_version` field tracks format changes; today only `1` exists.

### Key persistence

The signing keypair lives on disk at `TKR_RECEIPT_SIGNING_KEY_PATH` (default `/var/lib/tkr/receipt-signing-key`). On first start, tkr-server mints + persists it with `0600` perms. On subsequent starts, the same key loads → signatures stay verifiable across restarts.

**Caveat:** if the path's parent isn't writable (default Docker rootfs with no volume mount), tkr-server falls back to an **ephemeral in-memory key** and logs a warning at startup. Mount a volume at `/var/lib/tkr` for persistence in production.

---

## Dashboard panel reference

| Panel | Data source | What's true |
|---|---|---|
| **Token usage** | `/api/v1/llm/recent` (3s poll) | Last 256 calls (in-memory ring). After restart, history is lost unless `DATABASE_URL` is set. |
| **Filter · pre-flight** | `/api/v1/filter/stats` (10s poll) | Counters for every rule that has fired since the last restart. Empty table = "armed, nothing matched." |
| **Receipts · audit drain queue** | `/api/v1/llm/receipts/stats` (5s poll) | Live FIFO depth + drop counter. `drop > 0` = drainer is missing, fix urgently. The "drain now" button hits `POST /drain` and empties the queue. |
| **Mesh** | `/api/v1/mesh/status` (5s poll) | Tkr's mesh primitive — peers + enrolled members. Secondary feature, not the AI gateway story. |
| **Sandbox** | `/api/v1/sandbox/stats` (10s poll) | **Server-side HTTP endpoint only.** Off-default; flip `TKR_SANDBOX_EXEC=true` to enable. |
| **Captured calls** | `/api/v1/llm/captured` (5s poll) | Scrubbed request + response bodies. Off-default; flip `TKR_CAPTURE_BODIES=true` to enable. |
| **Ingested sessions** | `/api/v1/sessions` | CLI vault ingest path (separate from LLM-proxy traffic). POST a vault to `/api/v1/ingest` to populate. |

---

## Common gotchas

- **"My client setup but the dashboard's empty."** Confirm with `curl tkr.prysm.sh/api/v1/filter/stats` — if `total: 0` and `injections_total: 0`, the request never reached tkr-server (DNS / wrong `BASE_URL` / IDE process cached the old config — restart the IDE). If non-zero, tkr saw the request and you should also see entries on `/api/v1/llm/recent`.
- **"Why am I getting 429s?"** Two possible sources: upstream (Anthropic / OpenAI rate-limiting you — the `status` column on Token usage shows their 429) or tkr (`upstream_throttled > 0` in filter stats). If tkr, bump `TKR_UPSTREAM_MAX_CONCURRENT`.
- **"Receipts queue keeps growing."** Means no relayer is draining. Either click "drain now" manually, or stand up a process that polls `POST /api/v1/llm/receipts/drain` on a schedule and ships the batches to your audit sink (S3, SIEM, on-chain).
- **"Streaming-response credentials still get through sometimes."** Per-event scrubbing only sees one SSE event at a time. A credential that splits exactly across two upstream chunks would not match. Real models emit complete identifier tokens per delta, so this is rare in practice — but it's a known residual risk. See the streaming-scrub slice memo for details.

---

## Source of truth

When this doc and the code disagree, the code wins. Search points:

- Routes: `crates/tkr-server/src/main.rs` — search for `Method::POST,` to see every endpoint.
- Filter rules: `RedactionEngine::default_rules()` + `InjectionEngine::default_rules()`.
- Signing canonical format: `ReceiptSigner::canonical_message()`.
- Migration schema: `crates/tkr-server/migrations/*.sql`.
