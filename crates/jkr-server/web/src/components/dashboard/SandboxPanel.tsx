// Sandbox panel — pulls live counters from /api/v1/sandbox/stats.
//
// When the server has `JKR_SANDBOX_EXEC=true`, the panel shows real
// metrics (runs, denied, success rate, last command). When disabled
// (the default), it keeps the teaser copy describing what the
// sandbox does, with a small "armed: 0 runs" header so the panel
// doesn't read as broken.
//
// The "what it does" bullets stay visible in both modes — they
// describe the agent-side sandbox the CLI ships with, which is the
// load-bearing security story regardless of whether the operator has
// flipped on the server-side exec endpoint.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  api,
  type SandboxRecentResponse,
  type SandboxStats,
} from "../../api";
import { Panel } from "../Panel";
import { Stat } from "../Stat";
import { fmtRelative } from "../../lib/format";

interface Props {
  stats?: SandboxStats;
}

// Smoke-test the sandbox endpoint from the dashboard. Runs `echo` —
// it's allowlisted, side-effect-free, and proves the whole pipe
// (auth → allowlist → Landlock/sandbox-exec → counter increment) is
// hot. On success we invalidate sandbox-stats so the tiles update
// without waiting for the 10s poll.
interface SandboxExecResponse {
  ok: boolean;
  exit: number;
  stdout: string;
  stderr: string;
  duration_ms: number;
  truncated: boolean;
}

export function SandboxPanel({ stats }: Props) {
  const qc = useQueryClient();
  const enabled = stats?.enabled ?? false;
  const total = stats?.total ?? 0;
  const failed = stats?.failed ?? 0;
  const denied = stats?.denied ?? 0;
  const successPct = stats?.success_rate_pct;

  const smoke = useMutation<SandboxExecResponse, Error, void>({
    mutationFn: () =>
      api<SandboxExecResponse>("/api/v1/sandbox/exec", {
        method: "POST",
        body: JSON.stringify({ command: "echo", args: ["sandbox", "armed"] }),
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["sandbox-stats"] });
      qc.invalidateQueries({ queryKey: ["sandbox-recent"] });
    },
  });

  // Newest-first ring of recent runs from the server. Only polled
  // when the endpoint is enabled — keeps the dashboard quiet in
  // disabled mode.
  const recent = useQuery<SandboxRecentResponse>({
    queryKey: ["sandbox-recent"],
    queryFn: () => api<SandboxRecentResponse>("/api/v1/sandbox/recent"),
    refetchInterval: 10_000,
    enabled,
  });
  const runs = recent.data?.entries ?? [];

  // Header count reflects whether the operator has any data yet.
  const count = !stats
    ? "…"
    : enabled
      ? total === 0
        ? "armed · 0 runs"
        : `${total} runs · ${denied} denied`
      : "coming soon";

  return (
    <Panel
      title="sandbox"
      dot={enabled ? "live" : "off"}
      count={count}
      countClass={enabled ? undefined : "muted"}
    >
      {enabled ? (
        <>
          <div className="lp-stats lp-stats-3" style={{ padding: "12px" }}>
            <Stat value={String(total)} label="total runs" />
            <Stat value={String(failed)} label="failed" />
            <Stat value={String(denied)} label="denied · server-exec" />
            <Stat
              value={successPct != null ? `${successPct}%` : "—"}
              label="success rate"
            />
            <Stat
              value={stats?.last?.command ?? "—"}
              label="last command"
            />
            <Stat
              value={stats?.last ? fmtRelative(stats.last.ts) : "—"}
              label="last seen"
            />
          </div>
          <p className="sandbox-pitch" style={{ paddingTop: 0 }}>
            allowed: <code>{stats?.allowed_commands.join(", ")}</code>. each
            invocation runs in a per-request jail — no network, no env, fs
            read-only on loader paths.
          </p>
          <div className="sandbox-smoke">
            <button
              type="button"
              className="sandbox-smoke-btn"
              onClick={() => smoke.mutate()}
              disabled={smoke.isPending}
            >
              {smoke.isPending ? "running…" : "test sandbox"}
            </button>
            {smoke.data && (
              <span className="sandbox-smoke-out ok">
                exit {smoke.data.exit} · {smoke.data.duration_ms}ms ·{" "}
                <code>{smoke.data.stdout.trim() || "(no stdout)"}</code>
              </span>
            )}
            {smoke.error && (
              <span className="sandbox-smoke-out err-inline">
                {smoke.error.message}
              </span>
            )}
          </div>
          {runs.length > 0 && (
            <table>
              <thead>
                <tr>
                  <th>when</th>
                  <th>command</th>
                  <th className="num">ms</th>
                  <th className="num">exit</th>
                </tr>
              </thead>
              <tbody>
                {runs.slice(0, 8).map((r, i) => (
                  <tr key={`${r.ts}-${i}`}>
                    <td className="muted">{fmtRelative(r.ts)}</td>
                    <td>{r.command}</td>
                    <td className="num">{r.duration_ms}</td>
                    <td className={r.exit === 0 ? "ok num" : "err-inline num"}>
                      {r.exit}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      ) : (
        <div className="sandbox-teaser">
          <p className="sandbox-pitch">
            every agent session runs in a per-user jail — Landlock on Linux,
            sandbox-exec on macOS. no shared filesystem, no escaped processes,
            torn down with the task.
          </p>
          <ul className="sandbox-bullets">
            <li>filesystem · scoped to the agent&apos;s working tree</li>
            <li>network · explicit allow-list per session</li>
            <li>lifetime · ends when the task ends</li>
          </ul>
          <p className="sandbox-pitch" style={{ marginTop: 12, fontSize: 12 }}>
            jkr-server also ships a sandboxed-exec endpoint at{" "}
            <code>POST /api/v1/sandbox/exec</code>. flip{" "}
            <code>JKR_SANDBOX_EXEC=true</code> on the server to enable it.
          </p>
        </div>
      )}
    </Panel>
  );
}
