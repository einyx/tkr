// Mesh peers / enrollment — secondary panel on the dashboard since
// the AI gateway is the headline now. Kept because Tkr does ship the
// mesh primitive and the dashboard should still surface it.

import type { MeshStatus } from "../../api";
import { Panel } from "../Panel";

interface Props {
  status?: MeshStatus;
  error: unknown;
}

export function MeshPanel({ status, error }: Props) {
  return (
    <Panel
      title="mesh"
      count={
        status
          ? `${status.total_connected} online · ${status.total_meshes} mesh${status.total_meshes === 1 ? "" : "es"}`
          : "…"
      }
    >
      {error ? (
        <div className="err" style={{ padding: "24px" }}>
          {String(error)}
        </div>
      ) : !status ? (
        <div className="empty">loading…</div>
      ) : status.meshes.length === 0 ? (
        <div className="empty">
          no peers connected. run <code>tkr mesh join &lt;invite-url&gt;</code> on a host to
          appear here.
        </div>
      ) : (
        <table>
          <thead>
            <tr>
              <th>mesh id</th>
              <th className="num">enrolled</th>
              <th className="num">connected</th>
            </tr>
          </thead>
          <tbody>
            {status.meshes.map((m) => (
              <tr key={m.meshId}>
                <td>{m.meshId}</td>
                <td className="num">{m.enrolled}</td>
                <td className="num ok">{m.connected}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}
