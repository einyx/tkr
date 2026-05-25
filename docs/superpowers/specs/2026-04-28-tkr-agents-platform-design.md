# jkr Agents — Design Spec

**Status:** Draft
**Date:** 2026-04-28
**Owner:** @einyx

---

## 1. Summary

**jkr Agents** is a local-first, cost-metered, sovereign agent framework for infrastructure and SRE work. Users author agents in TOML (with Starlark for logic, Python SDK or Rust crates for power users), run them as a single Rust binary on their own box or in their own VPC, and pay for a hosted dashboard that turns fleet runs into cost receipts.

The product is built on three Rust primitives — the existing `jkr` filter library, a new `jkr-sandbox`, and a new `jkr-vault` — that collectively ensure an agent can do useful infra work without exfiltrating secrets, executing destructive actions outside its declared capabilities, or leaking raw telemetry to LLM providers.

## 2. Positioning

> *The agent framework that knows `kubectl logs` is 80% noise — and won't ship the 20% that's a private key.*

| | jkr Agents | VoltAgent | OpenSRE | LangGraph / Mastra |
|---|---|---|---|---|
| Domain focus | Infra / SRE | General | SRE | General |
| Shape | Framework + runtime | Framework + dashboard | Toolkit / libraries | Framework |
| Where it runs | Customer's box / VPC | Self-host or cloud | Anywhere | Anywhere |
| Cost-metering | First-class primitive | Observability feature | Not a focus | Not a focus |
| Sovereignty | Default | Optional | Default | Optional |

## 3. Audience

**Primary (v1):**
- **Indie devs / vibe coders** running coding agents and one-off SRE scripts. Pay $10–30/mo to cap personal token bills.
- **AI startups** building agent products on Anthropic/OpenAI. Pay per-seat / per-call to shrink agent COGS.

Both are bottom-up, dev-led, self-serve. Same product at different price tiers.

**Later (v2+):** ops teams, then enterprise compliance.

## 4. Wedge

In priority order:

1. **Ops/SRE-flavored framework.** Typed first-class tools for `kubectl`, `docker`, `journald`, `gh`, `git`, `shell`, HTTP — each with built-in `jkr` filtering. We do not compete for "build a chatbot."
2. **Local-first / sovereign.** Single static Rust binary. Runs on the user's box or inside their VPC. No telemetry leaves unless opted in per agent.
3. **Cost-native.** Token savings are a primitive, not an afterthought. Every run produces a cost receipt; every dashboard view shows tokens-saved and dollars-saved.

## 5. Vision arc

Same Rust core throughout. Each ring is the same binary doing more.

1. **v1 — Agents.** Single-agent runs, manual + cron triggers, 6 typed tools, hosted dashboard with metered billing.
2. **v2 — Workflows.** Multi-agent orchestration. Webhook / Slack / Alertmanager triggers. Egress proxy mode. Python SDK GA. jkr becomes "Temporal for AI ops work."
3. **v3 — Platform.** Fleet control plane across local boxes, VPCs, edge. Tool/filter marketplace. RBAC, audit, SOC2. Agent ↔ agent capability tokens.

## 6. Authoring surfaces

Concentric rings around one Rust core:

- **Rust core** — the runtime, the binary, foundation.
- **Rust crates (D)** — public API for power users to build typed tools, filters, vault adapters.
- **TOML + Starlark (E)** — primary surface for 90% of users. TOML manifest declares tools, capabilities, providers, mode. Starlark provides escape-hatch logic. v1 ships TOML; Starlark v1.1.
- **Python SDK (C)** — optional PyO3 binding. v2 GA. Lets SRE/data folks write tool implementations and agent logic in Python.

We do NOT ship a TypeScript SDK in v1. Audience is infra, not frontend.

## 7. Architecture

### 7.1 Deployment modes

Single binary, two modes:

- **Local mode (v1).** `jkr agent run agent.toml`. Long-lived daemon optional (`jkr daemon start`) for cron + scheduled runs. Unix socket IPC, file mode 0600, no TCP listener by default.
- **Clustered mode (v2).** Same binary as a workload runtime in a customer Kubernetes / VPC. Agents become workloads, scheduled by a jkr orchestrator. Code paths feature-flagged off in v1 but the abstractions ship from day one.

### 7.2 Three load-bearing security primitives

#### `jkr` egress filter *(existing crate, promoted)*
The same filter library that powers the `jkr` CLI is the **single egress chokepoint** for tool output. Every tool result passes through `jkr` before reaching:
- the model (compression + redaction)
- the local run history (compression)
- the hosted dashboard (compression + extra redaction pass)

Filters are typed per tool. Built-in redaction matches AWS/GCP/Azure keys, GH/GitLab tokens, JWTs, PEM blocks, `.env` assignments, kubeconfig client-cert blobs, datadog/grafana/PagerDuty API keys. Filter rules ship as Rust crates and as `*.star` files. Same code path produces the token-savings number.

#### `jkr-sandbox`
Every tool execution runs in a real sandbox in v1. No flag to disable; debug mode logs what *would* have been blocked.

- **Linux:** `landlock` (FS allowlist) + `seccomp-bpf` (syscall allowlist per tool class) + user namespaces (UID isolation) + cgroups v2 (CPU/mem/PID caps). No `CAP_NET_RAW`, no `CAP_SYS_ADMIN`.
- **macOS:** `sandbox_init` with per-tool `.sb` profiles.
- **Per-tool profiles.** `kubectl` reads `~/.kube`, network to API server, FS write only in `/tmp/jkr-run-<id>`. `docker`, `shell`, `gh` each have their own profile.
- **Network egress denied by default.** Opened by typed-tool declaration, per-host where possible.
- Profile is computed from the manifest at load time and signed into the run record.
- If a tool can't run sandboxed on the current OS → fail closed.

#### `jkr-vault`
Sovereign capability-based secret store. Agents never see raw secrets.

- **Storage.** Sealed-at-rest file (`~/.jkr/vault.kdb` or `/var/lib/jkr/vault.kdb`) using age, sealed by a hardware-bound key (Secure Enclave on macOS, TPM 2.0 on Linux, OS keychain fallback).
- **Memory.** Unsealed only in memory, `mlock`'d, zeroize-on-drop, never paged to swap.
- **Capability handles.** Manifest declares `vault_ref = "kube/prod-cluster"`. Runtime materializes the secret into the tool sandbox as a read-only tmpfs file at execution time, unmounts on completion. Agent and model never see the blob.
- **Defense-in-depth.** `jkr` egress filter independently scans tool output for vault material; redacts if any leaks.
- **Audit log.** Every materialization logged locally; mirrored to dashboard if telemetry opted in.

**v1 sources:** native sealed vault (default), 1Password CLI, `pass`, `gh secret`, env vars, AWS/GCP secret managers (read-only adapters).
**v1.5:** HashiCorp Vault adapter.
**v2+:** distributed cluster vault with mTLS between daemons, Shamir-shared root key. HSM-backed unseal for enterprise.

### 7.3 Component layout

```
jkr/
├── crates/
│   ├── jkr-core/        # existing filter library; the egress primitive
│   ├── jkr-cli/         # existing CLI; absorbs `jkr agent` subcommands
│   ├── jkr-agent/       # NEW — agent runtime: TOML loader, run loop, model client
│   ├── jkr-sandbox/     # NEW — landlock/seccomp/sandbox_init wrappers
│   ├── jkr-vault/       # NEW — sealed vault, capability handles, source adapters
│   ├── jkr-tools/       # NEW — typed tool implementations (kubectl, docker, etc.)
│   ├── jkr-providers/   # NEW — LLM provider adapters (Anthropic, OpenAI, local)
│   └── jkr-daemon/      # NEW — long-lived daemon, cron scheduler, IPC server
├── dashboard/           # NEW — hosted Next.js app: cost receipts, fleet view, billing
└── docs/
```

### 7.4 Data flow per agent run

```
TOML manifest
    │
    ▼
jkr-agent loader ──► jkr-vault (resolve capability handles)
    │
    ▼
loop:
    model.next() ──► tool_call
        │
        ▼
    jkr-sandbox.execute(tool, args, capabilities)
        │
        ▼
    raw_output (50KB)
        │
        ▼
    jkr-core filter ──► compressed + redacted output (800B)
        │
        ▼
    tool_result back to model
    │
    ▼
final answer + run receipt (tokens used, tokens saved, $-saved)
    │
    ▼
local run log (always) + dashboard (if opted in, metadata only by default)
```

### 7.5 v1 default posture

- Agent mode default: `approve` (human-in-the-loop on every mutating call). `auto` requires per-tool allowlists.
- Sandbox: **always on**, no disable flag.
- Vault: **always on**; even zero-secret agents get a vault context.
- Egress filter: **always on** to the model; required for redaction.
- Telemetry to dashboard: **opt-in, off by default**, agent-by-agent. Metadata-only when on; tool inputs/outputs and prompts never leave the box unless explicitly enabled per agent.
- Daemon: unix socket only, no TCP listener by default.
- Manifests must be signed for `auto` mode; unsigned allowed for `approve`/`dry-run` so devs can iterate.

## 8. v1 scope

**In:**
- `jkr agent run <manifest.toml>` (foreground)
- `jkr daemon start` + cron-triggered runs
- 6 typed tools: `kubectl`, `docker`, `journald`, `gh`, `git`, `shell`
- 2 LLM providers: Anthropic, OpenAI
- `jkr-sandbox` on Linux + macOS
- `jkr-vault` with native sealed vault + 1Password / pass / gh-secret / env / AWS/GCP secret manager adapters
- Manifest signing (sigstore/cosign)
- Hosted dashboard: per-tenant fleet view, run timeline, token-savings + cost analytics, Stripe metered billing (free tier + Pro $20/mo).

**Explicitly out (v2+):**
- Multi-agent orchestration (single agent per run in v1)
- Webhook / Slack / Alertmanager triggers (manual + cron only in v1)
- Egress proxy mode for arbitrary LLM SDKs
- Python SDK (Rust + TOML only)
- Starlark logic (TOML-only manifests in v1; Starlark in v1.1)
- Marketplace, RBAC, audit log UI
- HashiCorp Vault, HSM, FIPS
- TypeScript SDK
- Windows support

## 9. Monetization

**Free / OSS (MIT, Rust core):**
- `jkr` CLI, `jkr agent run`, all three security primitives, all v1 tools and providers, local-only operation forever.

**Paid (hosted dashboard):**
- Free tier: up to 200 runs/month, 7-day retention, 1 user.
- Pro $20/mo: unlimited runs, 90-day retention, team features, alerting on agent failures, slack integration.
- Team / Enterprise (v2+): SSO, RBAC, longer retention, audit export, SOC2.

The OSS binary stays fully usable without ever talking to the dashboard. Local users never see a paywall.

## 10. Distribution

- **Open source first.** Public on GitHub day one. MIT.
- **Launch surface:** Hacker News (Show HN), r/sre, r/kubernetes, r/devops, lobste.rs.
- **Design partners:** target 10 SRE-leaning indie hackers and 3 AI-startup ops teams pre-launch.
- **Demo gravity:** the killer demo is `kubectl logs` against a real cluster — model gets 800B of distilled signal, dashboard shows $0.04 saved, run receipt proves it.

## 11. Top risks

| Risk | Mitigation |
|------|------------|
| Agent framework category is brutal; we get drowned by Volt/Mastra/LangGraph | Don't fight them — own SRE/infra niche, don't ship a chatbot story at all |
| Sandboxing is platform-specific and slow to land cleanly on macOS | Ship Linux first-class; macOS best-effort with documented gaps; treat WSL2 as the Windows story |
| `jkr-vault` is a security product and we are not a security company | Lean on age/cosign/landlock — established primitives. No bespoke crypto. External audit before any enterprise sale. |
| Hosted dashboard is meaningful backend work | v1 dashboard is a thin Next.js + Postgres + Stripe stack. Don't multi-tenant at infra level in v1 — row-level isolation is enough for paid Pro. |
| Prompt injection via tool output leads to a destructive action despite sandbox | `approve` mode is the v1 default; `auto` requires explicit per-pattern allowlists; mutation-vs-read is hard-coded into typed tools. |
| Token-savings claims are challenged | Ship the receipt: every run logs raw-bytes-in, filtered-bytes-in, model-tokens-charged, all client-verifiable. |

## 12. Open questions

1. Naming — keep "jkr Agents" or rebrand the agent product (`jkr` stays the CLI/library)?
2. Dashboard host — start on Vercel/Fly/Render, or self-host from day one to align with the sovereignty pitch?
3. Pricing meter — per agent-run, per token-saved, or flat tier? Per-agent-run is the simplest invoice; per-token-saved aligns incentives but is harder to bill.
4. Do we publish a public filter/tool registry in v1, or hold for v2 to avoid supply-chain headaches?
5. Hardware-bound vault key on Linux servers without TPM — fall back to OS keychain or require explicit unseal-passphrase mode?

---

*End of spec.*
