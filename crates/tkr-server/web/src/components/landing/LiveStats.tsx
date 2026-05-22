import type { LlmCallReceipt } from "../../api";
import { Stat } from "../Stat";
import { fmtTokens } from "../../lib/format";
import { RecentCallsTable } from "./RecentCallsTable";

interface Props {
  entries: LlmCallReceipt[];
}

export function LiveStats({ entries }: Props) {
  const calls = entries.length;
  const inTok = entries.reduce((a, e) => a + (e.input_tokens || 0), 0);
  const outTok = entries.reduce((a, e) => a + (e.output_tokens || 0), 0);
  const avg =
    calls > 0
      ? Math.round(entries.reduce((a, e) => a + (e.duration_ms || 0), 0) / calls)
      : null;
  const last = entries[0];
  const isLive = calls > 0;

  return (
    <section id="live" className="lp-section lp-live lp-reveal lp-reveal-delay-1">
      <div className="lp-section-head">
        <h2 className="lp-section-title">live · this instance</h2>
        <span className={`lp-live-badge${isLive ? " lp-live-badge-on" : ""}`}>
          <span className="status-dot live" aria-hidden="true" />
          {isLive ? "streaming" : "waiting for traffic"}
        </span>
      </div>
      <p className="lp-section-body">
        Real calls through this gateway — not a mock. Polls every few seconds.
      </p>
      <div className="lp-stats lp-stats-live">
        <Stat value={String(calls)} label="calls in buffer" />
        <Stat value={fmtTokens(inTok)} label="input tokens" />
        <Stat value={fmtTokens(outTok)} label="output tokens" />
        <Stat value={avg != null ? `${avg}ms` : "—"} label="avg latency" />
        <Stat value={last?.model ?? "—"} label="last model" className="lp-stat-wide" />
        <Stat
          value={last?.status != null ? String(last.status) : "—"}
          label="last status"
        />
      </div>
      <RecentCallsTable entries={entries.slice(0, 6)} />
    </section>
  );
}
