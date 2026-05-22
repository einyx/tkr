# Pointing your AI tools at tkr

This is what you do once a tkr instance is running (your own deployment
or `tkr.prysm.sh`). Five minutes from "I have the URL" to "my dashboard
shows traffic."

---

## What tkr is doing for you

You point your coding agent's base URL at tkr instead of straight at
Anthropic / OpenAI. From there:

- The request body is **scanned for credentials** before it leaves your
  boundary. AWS access keys, GitHub PATs, OpenAI / Anthropic keys,
  Slack tokens, JWTs — replaced with `[REDACTED:<rule>]` markers. The
  provider never sees the raw secret.
- **Prompt-injection** patterns ("ignore previous instructions", DAN
  jailbreaks, role-overwrite attempts) on user turns are logged.
  Blocking is opt-in per rule (off by default — false positives hurt).
- The proxy is **wire-compatible** with Anthropic Messages and OpenAI
  Chat Completions, streaming included, header semantics preserved.
- Every call records a **signed receipt** (model, tokens, latency,
  status) on a drain queue your relayer can poll.
- Sign-in is **Logto-backed SSO** — same identity as the rest of the
  Prysm stack. The dashboard is just for operators; client traffic
  doesn't need a tkr account.

What tkr is *not* doing (yet — see [Known gaps](#known-gaps)):

- It does not yet scrub **response** bodies. If a model echoes back a
  secret it was given (e.g. in a system prompt), tkr passes the response
  through verbatim.
- It does not yet enforce **per-user / per-tenant quotas**. There is a
  global concurrency cap (see below) but no per-key throttling.
- The receipt signature path is **in-process audit-only** for now;
  on-chain settlement via MeshEscrow is wired but not yet flipped on.

---

## Point your tool at it

Three flags do all the work: an environment variable on the client
side, and the credential the client already has for the provider. tkr
never sees, stores, or substitutes your provider keys — the
`x-api-key` / `Authorization: Bearer` header is relayed verbatim.

### Claude Code

```bash
export ANTHROPIC_BASE_URL=https://tkr.prysm.sh
export ANTHROPIC_API_KEY=sk-ant-…  # your existing Anthropic key

# then just use Claude Code as normal:
claude
```

Streaming, tool use, prompt caching, vision — all preserved.

### Cursor

Cursor's settings → "Models" → "OpenAI API Key" → "Base URL".

```
Base URL:   https://tkr.prysm.sh/v1
API Key:    sk-…   (your existing OpenAI key)
```

For Cursor's Anthropic backend, the same idea using `ANTHROPIC_BASE_URL`
in the env it inherits.

### Codex (OpenAI CLI)

```bash
export OPENAI_BASE_URL=https://tkr.prysm.sh/v1
export OPENAI_API_KEY=sk-proj-…

codex …
```

### Any Anthropic / OpenAI SDK app

The SDKs both read `*_BASE_URL` from the environment, or you pass it
to the constructor. Identical to the above.

### Raw `curl` smoke

The fastest way to confirm tkr is in your path:

```bash
# Anthropic-wire
curl https://tkr.prysm.sh/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -H "content-type: application/json" \
  -d '{
    "model": "claude-sonnet-4-6",
    "max_tokens": 64,
    "messages": [{"role":"user","content":"reply with the word OK"}]
  }'

# OpenAI-wire
curl https://tkr.prysm.sh/v1/chat/completions \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H "content-type: application/json" \
  -d '{
    "model": "gpt-4o-mini",
    "messages": [{"role":"user","content":"reply with the word OK"}]
  }'
```

Within a few seconds, the call shows up on the dashboard's "token
usage" panel.

---

## What you see in the dashboard

Sign in via "Continue with Prysm →" on the landing. The dashboard has
six panels:

1. **token usage** — last 256 calls in a rolling buffer with input /
   output tokens, latency, last model + status. A 12-row table of the
   most recent calls below.
2. **filter · pre-flight** — every redaction or injection rule that has
   fired since the last server restart. Empty table reads "armed · 0
   hits" — that's the healthy idle state.
3. **receipts · audit drain queue** — depth of the FIFO of receipts
   waiting for a relayer to drain via `POST /api/v1/llm/receipts/drain`.
   `dropped (drainer behind)` going non-zero is your "wire a drainer"
   signal.
4. **sandbox · agent-reported** — placeholder. Agent-side sandbox
   metrics aren't ingested yet.
5. **mesh** — secondary. Tkr's mesh primitive (peers, enrollments).
6. **ingested sessions** — secondary. Session vaults POSTed to
   `/api/v1/ingest`.

---

## What happens on a redaction / injection hit

**Redaction (pre-flight):** the matched span is replaced with
`[REDACTED:<rule-name>]` *before* the body is forwarded. The
**model never sees the original secret**. The model will see a
sentence like:

> "deploy with `[REDACTED:aws-access-key]` please"

…which is enough context to keep the conversation coherent without
exposing the credential. The `redactions` counter on
`/api/v1/filter/stats` bumps once per request per rule (not once per
match). The dashboard's filter panel shows the breakdown.

**Injection (pre-flight, log mode):** the request flows through
unchanged. The injection counter increments. No 4xx is returned. This
is deliberate — false-positives on injection patterns are common, and
auto-blocking would catch legitimate user phrases.

**Injection (pre-flight, block mode):** opt-in per rule on the server
side. When a rule with `InjectionAction::Block` matches, tkr returns:

```http
HTTP/1.1 400 Bad Request
content-type: application/json

{"error":{"code":"prompt_injection_blocked","message":"request blocked by injection rule: <name>"}}
```

The client never reaches the upstream. The `injections_blocked` counter
bumps.

---

## Security model in one paragraph

tkr is a man-in-the-middle by design between your agents and the
model providers. It sees every request and response. It does **not**
see your provider API key — the `x-api-key` / `Authorization: Bearer`
header you set on the client is relayed to upstream verbatim and not
logged. It **does** see the prompts themselves (it has to, in order to
filter them). For self-hosted deployments this is fine because the
prompts never leave a boundary you control. For the hosted instance at
`tkr.prysm.sh`, treat it accordingly: it is operationally trusted by
the Prysm team, not cryptographically isolated.

---

## Concurrency + rate-limit defaults

tkr-server caps **concurrent in-flight upstream calls** to
`TKR_UPSTREAM_MAX_CONCURRENT` (default 64). Above the cap, requests
return `429 Too Many Requests` with `Retry-After: 1`. This protects the
blocking-thread pool from a runaway client; it is not a substitute for
per-tenant rate limiting at your edge.

For real production traffic, fronting tkr with traefik or nginx gets
you per-IP + per-key throttling cheaply. A traefik snippet:

```yaml
http:
  middlewares:
    tkr-ratelimit:
      rateLimit:
        average: 10            # rps per source IP (sustained)
        burst: 30              # short-burst headroom
        sourceCriterion:
          requestHeaderName: Authorization   # rate-limit per API key, not per IP
  routers:
    tkr:
      rule: Host(`tkr.example.com`)
      middlewares: [tkr-ratelimit]
      service: tkr-server
```

---

## Known gaps (for prospective deployers)

| Gap | Workaround until fixed |
|---|---|
| No response-side scrubbing — model echoes of secrets pass through | Don't put secrets in system prompts. |
| No server-side signing of receipts | Treat receipts as audit-only, not legally non-repudiable. |
| No per-tenant filter rule overrides | Run separate tkr instances per trust boundary if your teams need different rulesets. |
| Sandbox metrics not ingested into the dashboard yet | The sandbox is still active agent-side; the dashboard just doesn't surface it. |
| Async hyper-rustls client not in (uses blocking ureq under spawn_blocking) | Above `TKR_UPSTREAM_MAX_CONCURRENT` you get 429s instead of resource exhaustion — acceptable until streaming concurrency starts mattering. |

---

## Running tkr yourself

If you're operating a tkr instance (rather than just integrating against
`tkr.prysm.sh`), [`operations.md`](operations.md) covers env flags, the
two sandboxes (CLI vs server-side HTTP), receipt-signature verification,
and the dashboard panel reference.

---

## Where to file things

- Issues / feature requests: https://github.com/einyx/tkr
- Hosted instance: `tkr.prysm.sh` — sign in with your Prysm identity
- The umbrella stack: https://prysm.sh
