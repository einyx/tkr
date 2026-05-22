// Ingested CLI/agent session vaults — the original tkr-session
// content. Kept as a secondary panel for operators who already ship
// vaults to /api/v1/ingest; the row click opens SessionDetail.

import type { SessionMeta } from "../../api";
import { Panel } from "../Panel";

interface Props {
  sessions?: SessionMeta[];
  error: unknown;
  onSelect: (id: string) => void;
}

export function SessionsPanel({ sessions, error, onSelect }: Props) {
  return (
    <Panel title="ingested sessions" count={sessions?.length ?? "…"}>
      {error ? (
        <div className="err" style={{ padding: "24px" }}>
          {String(error)}
        </div>
      ) : !sessions ? (
        <div className="empty">loading…</div>
      ) : sessions.length === 0 ? (
        <div className="empty">
          no sessions yet — POST a vault to <code>/api/v1/ingest</code> to populate.
        </div>
      ) : (
        <table>
          <thead>
            <tr>
              <th>id</th>
              <th>agent</th>
              <th>started</th>
              <th>tkr</th>
            </tr>
          </thead>
          <tbody>
            {sessions.map((s) => (
              <tr
                key={s.session_id}
                className="session-row"
                onClick={() => onSelect(s.session_id)}
              >
                <td>{s.session_id}</td>
                <td className="muted">{s.agent}</td>
                <td className="muted">{s.started_at}</td>
                <td className="muted">{s.tkr_version}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}
