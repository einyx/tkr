// Audit drain queue — the FIFO of LlmCallReceipts waiting to be
// shipped to an external relayer. Dashboard surfaces queue depth,
// readiness flags, and (loudly) the cumulative drop count when the
// drainer is falling behind.
//
// The "drain now" button hits the existing POST /api/v1/llm/receipts/drain
// endpoint, completes the relayer round-trip from the UI, and refreshes
// the stats so the operator can see the counter drop in real time.
// Useful for demos + for one-shot manual drains during incident review.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api, type LlmReceiptStats } from "../../api";
import { Panel } from "../Panel";
import { Stat } from "../Stat";
import { fmtRelative } from "../../lib/format";

interface Props {
  stats?: LlmReceiptStats;
}

interface DrainResponse {
  ts: number;
  count: number;
  drained: unknown[];
}

export function ReceiptsPanel({ stats }: Props) {
  const qc = useQueryClient();
  const drain = useMutation<DrainResponse, Error, void>({
    mutationFn: () =>
      api<DrainResponse>("/api/v1/llm/receipts/drain", { method: "POST" }),
    onSuccess: () => {
      // Receipts queue depth dropped to zero; refresh the stats so
      // the operator sees the change without waiting for the 5s poll.
      qc.invalidateQueries({ queryKey: ["llm-receipt-stats"] });
    },
  });

  const pending = stats?.total ?? 0;

  return (
    <Panel
      title="receipts · audit drain queue"
      dot={stats?.readyToDrain ? "warn" : "live"}
      count={
        stats
          ? stats.readyToDrain
            ? "ready to drain"
            : `${stats.total} pending`
          : "…"
      }
    >
      {!stats ? (
        <div className="empty">loading…</div>
      ) : (
        <div className="lp-stats lp-stats-3" style={{ padding: "12px" }}>
          <Stat value={String(stats.total)} label="pending" />
          <Stat value={String(stats.batchSize)} label="batch size" />
          <Stat value={`${stats.maxAgeSecs}s`} label="max age" />
          <Stat
            value={stats.oldestQueuedAt != null ? fmtRelative(stats.oldestQueuedAt) : "—"}
            label="oldest queued"
          />
          <Stat value={String(stats.queueCap)} label="queue cap" />
          <Stat
            value={String(stats.totalDropped)}
            label="dropped (drainer behind)"
          />
        </div>
      )}

      <div className="sandbox-smoke">
        <button
          type="button"
          className="sandbox-smoke-btn"
          onClick={() => drain.mutate()}
          disabled={drain.isPending || pending === 0}
          title={
            pending === 0
              ? "no receipts to drain"
              : "drain the queue and return the batch"
          }
        >
          {drain.isPending ? "draining…" : `drain now${pending > 0 ? ` (${pending})` : ""}`}
        </button>
        {drain.data && (
          <span className="sandbox-smoke-out ok">
            drained {drain.data.count} receipt{drain.data.count === 1 ? "" : "s"}
            {" · "}
            <span className="muted">{fmtRelative(drain.data.ts)}</span>
          </span>
        )}
        {drain.error && (
          <span className="sandbox-smoke-out err-inline">
            {drain.error.message}
          </span>
        )}
      </div>

      {stats && stats.totalDropped > 0 ? (
        <div className="err" style={{ padding: "12px 16px" }}>
          drainer is falling behind: {stats.totalDropped} receipt
          {stats.totalDropped === 1 ? "" : "s"} dropped since last restart. point an
          external relayer at <code>POST /api/v1/llm/receipts/drain</code>.
        </div>
      ) : null}
    </Panel>
  );
}
