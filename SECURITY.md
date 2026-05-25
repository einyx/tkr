# Security policy

## Disclosure

If you find a vulnerability in jkr, **please don't open a public issue
or PR with the details.** Email `security@prysm.sh` (or, if that
bounces, open a private GitHub Security Advisory on the repository).
We aim to acknowledge within 48 hours and ship a fix in the next
release cycle.

Include in your report:

- A description of the vulnerability and its impact.
- Reproduction steps or a proof-of-concept (please keep it minimal —
  no public exploitation, no scraping for victims).
- The git SHA / release tag you tested against.
- Any suggested mitigation.

We'll credit you in the release notes if you want it. We don't run a
bug-bounty program right now; we do hand out polite acknowledgement
and beer at conferences.

---

## Trust model

Tkr-server is **a man-in-the-middle by design** between your AI
agents and the model providers (Anthropic, OpenAI). When you point a
client at jkr, the following are true:

- The client's **provider API key** (Anthropic `x-api-key`, OpenAI
  `Authorization: Bearer`) is **relayed to upstream verbatim** and
  **not logged or stored** by jkr-server.
- The **prompts themselves** pass through jkr-server in cleartext.
  Tkr has to read them to scrub credentials and run injection
  heuristics. For self-hosted deployments this is acceptable
  because the prompts never leave a boundary you control. For the
  hosted instance at `tkr.prysm.sh`, you're trusting the Prysm
  team operationally — there's no cryptographic isolation.
- **Body capture is off by default**
  (`JKR_CAPTURE_BODIES=false`). When off, jkr keeps a rolling
  receipt ring (metadata only: model, tokens, status, latency,
  signature) but does not persist prompt bodies. When operators
  flip capture on, scrubbed bodies are stored in a per-instance
  ring buffer.
- **Receipts are signed** with secp256k1 ECDSA. The signing key
  lives on the jkr-server host
  (`JKR_RECEIPT_SIGNING_KEY_PATH`). Verifying parties only need
  the receipt's `signer_pubkey` field plus the canonical-message
  format documented in `docs/operations.md`.

### What jkr does NOT defend against

- **A malicious jkr-server operator.** If you run jkr yourself,
  you're trusted; if you use the hosted instance, you're trusting
  Prysm operationally.
- **A compromised model provider** (Anthropic, OpenAI). Tkr can't
  protect prompts from upstream — it relays them. Customer
  data-handling at the model provider is governed by the
  provider's own terms.
- **A compromised host kernel / hypervisor.** Same as any
  application.

### What jkr defends against

- **Operator credentials leaking to providers.** Pre-flight
  redaction (AWS keys, GitHub PATs, OpenAI / Anthropic keys, Slack
  tokens, JWTs) catches accidental paste of secrets in prompts
  before the bytes leave the boundary.
- **Model echoing secrets back.** Response-side redaction (both
  buffered and streaming) rewrites known credential patterns in
  upstream responses before the client sees them.
- **Tool-call escape on the server-side sandbox endpoint.** The
  `POST /api/v1/sandbox/exec` endpoint runs allowlisted binaries
  under Landlock (Linux) or sandbox-exec (macOS) with no network,
  empty env, read-only loader paths, and hard CPU / memory /
  output / timeout caps.
- **Prompt-injection in user-supplied content.** The
  injection-heuristic engine logs (and optionally blocks) known
  jailbreak prefixes seen on user-role messages.
- **Receipt forgery.** Every receipt is signed; replaying or
  fabricating one without the server's private key is a hard
  cryptographic problem.

---

## In-scope for vulnerability reports

- Anything in `~/jkr/crates/jkr-server/` (the HTTP server +
  proxy + filter + signing + sandbox).
- Anything in `~/jkr/crates/jkr-sandbox/` (the Landlock /
  sandbox-exec wrapper).
- Anything in `~/jkr/crates/jkr-mesh/` (mesh + broker + EIP-712
  invite-verification).
- The deployed instance at `tkr.prysm.sh`.

## Out of scope

- Findings that require root / admin access to the host already
  (post-compromise persistence, sidechannel inside the same VM).
- Volumetric DoS (we have edge rate-limiting; that's an ops
  concern, not a code concern).
- Issues in third-party dependencies that have a published CVE +
  upstream fix — open a regular PR bumping the dep version.
- Findings in the example / demo wallet code under
  `crates/jkr-server/web/` that don't affect server behaviour.
- Social-engineering findings against tkr.prysm.sh (we treat the
  hosted instance like any other SaaS).

---

## Cryptographic primitives

| Use | Primitive | Library |
|---|---|---|
| Receipt signing | secp256k1 ECDSA over SHA-256 (compact 64-byte sigs, 33-byte compressed pubkeys) | [`k256`](https://docs.rs/k256) |
| Logto OIDC PKCE | S256 (SHA-256 of verifier) | [`sha2`](https://docs.rs/sha2) + [`base64`](https://docs.rs/base64) |
| Session IDs / PKCE verifier | 32 random bytes via `OsRng` | [`rand`](https://docs.rs/rand) |
| Mesh invites | EIP-712 signatures, secp256k1 | `jkr-mesh` (see crate docs) |
| TLS to upstream | rustls (via `ureq`'s default features) | [`ureq`](https://docs.rs/ureq) |

If you spot a misuse of any of these (wrong nonce reuse, missing
randomness, etc.), that's exactly the kind of report we want.

---

## Known operational caveats

These aren't vulnerabilities, but they affect the security posture
of any deployment:

- **Signing key is ephemeral without a volume mount.** Default
  `JKR_RECEIPT_SIGNING_KEY_PATH=/var/lib/jkr/receipt-signing-key`
  isn't writable in stock containers — mount a volume there or
  signatures don't survive restarts. Tkr-server logs a loud warning
  at startup if persistence fails.
- **Pre-flight redaction is pattern-based.** A novel credential
  format that doesn't match any rule will not be caught. The
  ruleset lives in `RedactionEngine::default_rules()`; add patterns
  there as needed.
- **Streaming response scrubbing is per-event.** A credential that
  splits exactly across two upstream SSE chunks could leak. Real
  models emit complete identifier tokens per delta in practice;
  this is a documented residual risk.
- **The server-side sandbox endpoint is opt-in but expands the
  attack surface.** Disable (`JKR_SANDBOX_EXEC=false`) unless you
  have a concrete use for it.

---

## Contact

- Email: `security@prysm.sh`
- Repository: <https://github.com/einyx/jkr>
- Operational docs: [`docs/operations.md`](docs/operations.md)
- Integration docs: [`docs/integration.md`](docs/integration.md)
