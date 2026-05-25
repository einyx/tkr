const CHIPS = [
  { label: "proxy", hint: "Anthropic + OpenAI compatible" },
  { label: "filter", hint: "60–95% fewer CLI tokens" },
  { label: "sandbox", hint: "Landlock / sandbox-exec jail" },
  { label: "receipts", hint: "Signed audit trail" },
] as const;

export function Hero(_props: { onSignIn?: () => void }) {
  return (
    <section className="lp-hero lp-reveal">
      <div className="lp-hero-glow" aria-hidden="true" />
      <div className="lp-eyebrow">
        <span className="status-dot live" aria-hidden="true" />
        Prysm · AI gateway
      </div>
      <h1 className="lp-title">Tkr</h1>
      <p className="lp-lead">
        The AI gateway for agentic engineering teams.
      </p>
      <p className="lp-subtitle">
        Drop-in proxy in front of Claude Code, Cursor, and Codex. Credentials
        scrubbed before requests leave your boundary. Tool calls run in an
        isolated jail. Every model call gets a signed receipt.
      </p>

      <ul className="lp-chips" aria-label="Core capabilities">
        {CHIPS.map((c) => (
          <li key={c.label} className="lp-chip" title={c.hint}>
            {c.label}
          </li>
        ))}
      </ul>

      <div className="lp-ctas">
        <a className="lp-cta lp-cta-primary" href="#install">
          get started
        </a>
        <a
          className="lp-cta lp-cta-secondary"
          href="https://github.com/einyx/jkr"
          target="_blank"
          rel="noopener noreferrer"
        >
          view on github
        </a>
      </div>

      <p className="lp-trust">
        Apache-2.0 · self-hosted · prompts never leave your boundary
      </p>
    </section>
  );
}
