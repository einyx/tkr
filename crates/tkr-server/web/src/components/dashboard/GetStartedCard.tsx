// First-run hero. Replaces the empty token-usage panel + dead stat
// tiles with one focused card: copyable env-var commands, a pulsing
// "waiting for traffic" indicator, and nothing else. As soon as the
// ring buffer has its first call, Dashboard.tsx swaps this out for
// the TokenUsagePanel hero.

import { useState } from "react";

export function GetStartedCard() {
  const host = typeof location !== "undefined" ? location.host : "tkr.prysm.sh";
  const anthropic = `ANTHROPIC_BASE_URL=https://${host}`;
  const openai = `OPENAI_BASE_URL=https://${host}/v1`;
  return (
    <section className="panel get-started">
      <div className="panel-head">
        <span>
          <span className="status-dot warn" />
          get started
        </span>
        <span className="count muted">waiting for first call…</span>
      </div>
      <div className="get-started-body">
        <p className="get-started-lede">
          point a client at this gateway and the dashboard wakes up. drop the
          env var into your shell, your CI, or your agent runner — whatever
          calls anthropic or openai sdks.
        </p>
        <div className="get-started-cmds">
          <CopyCmd label="anthropic SDK" cmd={anthropic} />
          <CopyCmd label="openai SDK" cmd={openai} />
        </div>
        <p className="get-started-hint">
          authentication, redaction, and receipts kick in automatically. the
          dashboard polls every 3s — your first call will show up on its own.
        </p>
      </div>
    </section>
  );
}

function CopyCmd({ label, cmd }: { label: string; cmd: string }) {
  const [copied, setCopied] = useState(false);
  const onCopy = () => {
    void navigator.clipboard.writeText(cmd).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };
  return (
    <div className="get-started-cmd">
      <div className="get-started-cmd-label">{label}</div>
      <div className="get-started-cmd-row">
        <code>{cmd}</code>
        <button type="button" onClick={onCopy} aria-label={`copy ${label} env var`}>
          {copied ? "copied" : "copy"}
        </button>
      </div>
    </div>
  );
}
