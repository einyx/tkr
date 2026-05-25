// Captured-bodies panel. Only useful when `JKR_CAPTURE_BODIES=true`
// on the server — otherwise renders an "off; flip the env" copy
// with no body table. When enabled, lists the most recent calls
// with click-to-expand <details> for the scrubbed request +
// response bodies.
//
// Rows are scoped to the logged-in owner server-side: a call is
// attributed to you only when it carries your CLI token (`x-jkr-token`,
// injected by `jkr sandbox claude`). So "armed · 0 captured" usually
// means you haven't run a token-attributed session yet, not that
// capture is broken — the empty state says as much and points at the
// CLI tokens panel.
//
// Types are defined locally rather than in api.ts because that file
// is currently root-owned (linter-locked); the local shape mirrors
// `LlmCapturedCall` on the server.

import { useQuery } from "@tanstack/react-query";
import { api } from "../../api";
import { Panel } from "../Panel";
import { fmtRelative } from "../../lib/format";

interface CapturedCall {
  ts: number;
  provider: string;
  model: string;
  status: number;
  input_tokens: number;
  output_tokens: number;
  duration_ms: number;
  streaming: boolean;
  request: string;
  response: string;
}

interface CapturedResponse {
  enabled: boolean;
  entries: CapturedCall[];
  capacity: number;
  max_body_bytes: number;
}

export function CapturedPanel() {
  const q = useQuery<CapturedResponse>({
    queryKey: ["llm-captured"],
    queryFn: () => api<CapturedResponse>("/api/v1/llm/captured"),
    refetchInterval: 5_000,
  });
  const data = q.data;
  const enabled = data?.enabled ?? false;
  const entries = data?.entries ?? [];

  const count = !data
    ? "…"
    : enabled
      ? entries.length === 0
        ? "armed · 0 yours"
        : `${entries.length} yours · cap ${data.capacity}`
      : "disabled";

  return (
    <Panel
      title="captured calls"
      dot={enabled ? "live" : "off"}
      count={count}
      countClass={enabled ? undefined : "muted"}
    >
      {!data ? (
        <div className="empty">loading…</div>
      ) : !enabled ? (
        <div className="empty">
          body capture is off. set{" "}
          <code>JKR_CAPTURE_BODIES=true</code> on the server to start
          stashing the scrubbed request + response bodies of every
          proxied call (ring of {data.capacity}, {Math.round(data.max_body_bytes / 1024)} KiB cap per side).
          off by default so the &ldquo;your prompts never leave&rdquo; claim
          stays honest on stock deploys.
        </div>
      ) : entries.length === 0 ? (
        <div className="empty">
          armed — but nothing is attributed to you yet. captured calls
          are scoped per-user: a call shows up here only when it carries
          your CLI token. mint one in the <strong>CLI tokens</strong>{" "}
          panel below and launch your agent with{" "}
          <code>jkr sandbox claude</code> — it injects the token as{" "}
          <code>x-jkr-token</code> so every proxied call lands in your
          bucket.
        </div>
      ) : (
        <>
          <div className="captured-actions">
            <button
              type="button"
              className="sandbox-smoke-btn"
              onClick={() => downloadJsonl(entries)}
              title="download all captured calls as JSON-lines (one call per line) for offline audit"
            >
              download as jsonl ({entries.length})
            </button>
            <span className="muted" style={{ fontSize: 11 }}>
              one call per line · scrubbed bodies · ring capped at{" "}
              {data.capacity}
            </span>
          </div>
          <div className="captured-list">
          {entries.slice(0, 12).map((e, i) => (
            <details key={`${e.ts}-${i}`} className="captured-entry">
              <summary className="captured-summary">
                <span className="muted">{fmtRelative(e.ts)}</span>
                <span>{e.provider}</span>
                <span className="muted">{e.model || "—"}</span>
                <span className="num">
                  {e.input_tokens === 0 && e.output_tokens === 0
                    ? "—"
                    : `${e.input_tokens} → ${e.output_tokens}`}
                </span>
                <span className={e.status >= 400 ? "err-inline" : "ok"}>
                  {e.status}
                </span>
                {e.streaming ? <span className="muted">stream</span> : null}
              </summary>
              <div className="captured-body">
                <div className="captured-label">request</div>
                <pre className="captured-pre">{e.request}</pre>
                <div className="captured-label">response</div>
                <pre className="captured-pre">{e.response}</pre>
              </div>
            </details>
          ))}
          </div>
        </>
      )}
    </Panel>
  );
}

/// Synthesise a JSON-lines blob from the captured ring and trigger a
/// browser download. One call per line keeps the file streamable
/// downstream (jq -c, less, awk) without having to parse a giant
/// JSON array.
function downloadJsonl(entries: CapturedCall[]) {
  const lines = entries.map((e) => JSON.stringify(e));
  const blob = new Blob([lines.join("\n") + "\n"], {
    type: "application/x-ndjson",
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  const stamp = new Date().toISOString().replace(/[:.]/g, "-");
  a.href = url;
  a.download = `jkr-captured-${stamp}.jsonl`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  // Browser revokes object URLs on next tick; cleanup explicit anyway.
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
