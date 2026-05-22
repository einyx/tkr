// Token-usage hero of the gateway dashboard. Aggregates the
// `/api/v1/llm/recent` ring buffer into a primary metric (calls in
// buffer) with a sparkline of per-call latency, three secondary
// stat tiles, and a recent-calls table. The ring is server-capped
// at 256 entries so reducers run on bounded input.
//
// Empty state is owned by Dashboard.tsx (it swaps in GetStartedCard
// when the buffer is empty), so this component renders unconditionally
// assuming `entries.length > 0`.

import type { LlmCallReceipt } from "../../api";
import { Panel } from "../Panel";
import { Stat } from "../Stat";
import { fmtRelative, fmtTokens } from "../../lib/format";

interface Props {
  entries: LlmCallReceipt[];
}

export function TokenUsagePanel({ entries }: Props) {
  const calls = entries.length;
  const inTok = entries.reduce((a, e) => a + (e.input_tokens || 0), 0);
  const outTok = entries.reduce((a, e) => a + (e.output_tokens || 0), 0);
  const avgMs = Math.round(
    entries.reduce((a, e) => a + (e.duration_ms || 0), 0) / calls,
  );
  const last = entries[0];

  return (
    <Panel
      title={`token usage · last ${calls} call${calls === 1 ? "" : "s"}`}
      count={`last: ${last.provider} · ${last.model}`}
    >
      <div className="tu-hero">
        <div className="tu-hero-metric">
          <div className="tu-hero-value">{calls}</div>
          <div className="tu-hero-label">calls in buffer</div>
        </div>
        <Sparkline entries={entries} />
      </div>
      <div className="lp-stats lp-stats-3 tu-secondary">
        <Stat value={fmtTokens(inTok)} label="input tokens" />
        <Stat value={fmtTokens(outTok)} label="output tokens" />
        <Stat value={`${avgMs}ms`} label="avg latency" />
      </div>
      <table>
        <thead>
          <tr>
            <th>when</th>
            <th>provider</th>
            <th>model</th>
            <th className="num">in</th>
            <th className="num">out</th>
            <th className="num">ms</th>
            <th>status</th>
          </tr>
        </thead>
        <tbody>
          {entries.slice(0, 12).map((e, i) => (
            <tr key={`${e.ts}-${i}`}>
              <td className="muted">{fmtRelative(e.ts)}</td>
              <td>{e.provider}</td>
              <td className="muted">{e.model || "—"}</td>
              <td className="num">{e.input_tokens}</td>
              <td className="num">{e.output_tokens}</td>
              <td className="num">{e.duration_ms}</td>
              <td className={e.status >= 400 ? "err-inline" : "ok"}>{e.status}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </Panel>
  );
}

// Inline SVG sparkline over per-call latency. We render newest-on-the-right
// (so the sparkline reads like a timeline) and scale to the max sample in
// the buffer — absolute ms doesn't matter for a glanceable trend.
function Sparkline({ entries }: { entries: LlmCallReceipt[] }) {
  const samples = entries.slice(0, 48).map((e) => e.duration_ms || 0).reverse();
  if (samples.length < 2) return null;
  const max = Math.max(...samples, 1);
  const w = 180;
  const h = 36;
  const step = w / (samples.length - 1);
  const path = samples
    .map((v, i) => `${i === 0 ? "M" : "L"} ${(i * step).toFixed(1)} ${(h - (v / max) * h).toFixed(1)}`)
    .join(" ");
  return (
    <svg className="tu-spark" viewBox={`0 0 ${w} ${h}`} width={w} height={h} aria-hidden>
      <path d={path} fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}
