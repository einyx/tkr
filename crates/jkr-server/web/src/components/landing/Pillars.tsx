interface PillarSpec {
  code: string;
  name: string;
  headline: string;
  body: string;
}

const PILLARS: PillarSpec[] = [
  {
    code: "01",
    name: "proxy",
    headline:
      "Wire-compatible Anthropic + OpenAI. Drop-in for Claude Code, Cursor, Codex.",
    body:
      "Point ANTHROPIC_BASE_URL or OPENAI_BASE_URL at this server and your agents flow straight through. Streaming SSE, header passthrough, error semantics preserved. Your prompts and your provider key never leave the boundary you deploy.",
  },
  {
    code: "02",
    name: "filter",
    headline: "Strips noise from CLI output. Scrubs credentials from every prompt.",
    body:
      "60–95% fewer tokens on git, npm, cargo, kubectl, docker — without touching your prompts. A pre-flight pattern engine catches AWS keys, GitHub PATs, OpenAI / Anthropic keys, Slack tokens, JWTs before they leave for any model provider.",
  },
  {
    code: "03",
    name: "sandbox",
    headline: "Every tool call in an isolated environment, torn down at end.",
    body:
      "Landlock on Linux, sandbox-exec on macOS. Each agent session scoped to the user who invoked it: no shared filesystem, no leaked env, no surprises in audit. The thing your security team has been asking for whenever someone says \"the agent runs shell commands.\"",
  },
  {
    code: "04",
    name: "receipts",
    headline: "Signed audit trail of every model call, drainable to your SIEM.",
    body:
      "Each request emits a receipt: provider, model, tokens, latency, status. Batched in-process for a /drain endpoint your relayer polls; optionally settled on-chain via MeshEscrow.sol on Base. For the first time, a number for \"what is the AI dev loop costing us.\"",
  },
];

export function Pillars() {
  return (
    <section id="pipeline" className="lp-section lp-reveal">
      <h2 className="lp-section-title">the pipeline</h2>
      <p className="lp-section-body">
        Four layers between your agents and the model providers — each one
        addressable on its own, all on by default.
      </p>
      <div className="lp-features lp-features-4">
        {PILLARS.map((p) => (
          <PillarCard key={p.code} {...p} />
        ))}
      </div>
    </section>
  );
}

function PillarCard({ code, name, headline, body }: PillarSpec) {
  return (
    <article className="lp-feature">
      <div className="lp-feature-title">
        <span className="lp-feature-code">{code}</span>
        {name}
      </div>
      <div className="lp-feature-headline">{headline}</div>
      <div className="lp-feature-body">{body}</div>
    </article>
  );
}
