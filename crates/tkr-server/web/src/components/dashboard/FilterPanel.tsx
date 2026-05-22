// Pre-flight filter activity — both engines surfaced in one table.
// Redactions rewrite content; injections log (or opt-in block). The
// /api/v1/filter/stats endpoint returns counters only for rules that
// have fired, so an empty table means "armed but nothing matched yet"
// rather than "no rules configured".

import type { FilterStats } from "../../api";
import { Panel } from "../Panel";

interface Props {
  stats?: FilterStats;
}

export function FilterPanel({ stats }: Props) {
  const redTotal = stats?.total ?? 0;
  const injTotal = stats?.injections_total ?? 0;
  const blocked = stats?.injections_blocked ?? 0;
  const grandTotal = redTotal + injTotal;

  const count = !stats
    ? "…"
    : grandTotal === 0
      ? "armed · 0 hits"
      : `${redTotal} redact · ${injTotal} inject${blocked > 0 ? ` (${blocked} blocked)` : ""}`;

  const rows: Array<{ name: string; kind: "redact" | "inject"; hits: number }> = [];
  if (stats) {
    for (const [name, hits] of Object.entries(stats.redactions)) {
      rows.push({ name, kind: "redact", hits });
    }
    for (const [name, hits] of Object.entries(stats.injections)) {
      rows.push({ name, kind: "inject", hits });
    }
    rows.sort((a, b) => b.hits - a.hits);
  }

  return (
    <Panel title="filter · pre-flight" count={count}>
      {!stats ? (
        <div className="empty">loading…</div>
      ) : grandTotal === 0 ? (
        <div className="empty">
          armed. redaction patterns: AWS keys, GitHub PATs, OpenAI/Anthropic keys,
          Slack tokens, JWTs. injection patterns: ignore-previous-instructions,
          DAN-style jailbreaks, role overwrite. nothing matched yet.
        </div>
      ) : (
        <table>
          <thead>
            <tr>
              <th>rule</th>
              <th>kind</th>
              <th className="num">hits</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={`${r.kind}-${r.name}`}>
                <td>{r.name}</td>
                <td className="muted">{r.kind}</td>
                <td className="num">{r.hits}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}
