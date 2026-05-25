// Compact recent-LLM-calls table for the public landing. Uses the
// grid-style markup (`.lp-llm-*`) instead of the Dashboard's
// `<table>` — different visual context, same underlying data shape.

import type { LlmCallReceipt } from "../../api";
import { fmtRelative } from "../../lib/format";

interface Props {
  entries: LlmCallReceipt[];
}

export function RecentCallsTable({ entries }: Props) {
  if (entries.length === 0) {
    return (
      <div className="lp-llm-empty">
        No calls yet. Point an Anthropic-SDK app at{" "}
        <code>ANTHROPIC_BASE_URL=https://{location.host}</code>{" "}
        and run a request — it&apos;ll show up here as it lands.
      </div>
    );
  }
  return (
    <div className="lp-llm-table">
      <div className="lp-llm-row lp-llm-row-head">
        <span>when</span>
        <span>model</span>
        <span className="lp-llm-num">in</span>
        <span className="lp-llm-num">out</span>
        <span className="lp-llm-num">ms</span>
        <span>status</span>
      </div>
      {entries.map((e, idx) => (
        <div key={`${e.ts}-${idx}`} className="lp-llm-row">
          <span className="lp-llm-time">{fmtRelative(e.ts)}</span>
          <span className="lp-llm-model">{e.model || "—"}</span>
          <span className="lp-llm-num">{e.input_tokens}</span>
          <span className="lp-llm-num">{e.output_tokens}</span>
          <span className="lp-llm-num">{e.duration_ms}</span>
          <span className={e.status >= 400 ? "lp-llm-bad" : "lp-llm-ok"}>
            {e.status}
          </span>
        </div>
      ))}
    </div>
  );
}
