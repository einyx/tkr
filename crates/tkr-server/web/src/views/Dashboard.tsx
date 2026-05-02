import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { api, type Me, type MeshStatus, type SessionMeta } from "../api";

interface Props {
  me: Me;
  onSelectSession: (id: string) => void;
  onSignedOut: () => void;
}

export function DashboardView({ me, onSelectSession, onSignedOut }: Props) {
  const qc = useQueryClient();

  const meshStatus = useQuery<MeshStatus>({
    queryKey: ["mesh-status"],
    queryFn: () => api<MeshStatus>("/api/v1/mesh/status"),
    refetchInterval: 3_000,
  });

  const sessionsQuery = useQuery<{ sessions: SessionMeta[] }>({
    queryKey: ["sessions"],
    queryFn: () => api<{ sessions: SessionMeta[] }>("/api/v1/sessions"),
  });

  const logout = useMutation({
    mutationFn: () => api("/api/auth/logout", { method: "POST" }),
    onSuccess: () => {
      qc.removeQueries();
      onSignedOut();
    },
  });

  return (
    <>
      <header>
        <div className="brand">tkr</div>
        <div className="who">
          {me.user.email || "anon"} ·{" "}
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault();
              logout.mutate();
            }}
          >
            sign out
          </a>
        </div>
      </header>
      <main>
        <MeshPanel status={meshStatus.data} error={meshStatus.error} />
        <SessionsPanel
          sessions={sessionsQuery.data?.sessions}
          error={sessionsQuery.error}
          onSelect={onSelectSession}
        />
      </main>
    </>
  );
}

function MeshPanel({
  status,
  error,
}: {
  status?: MeshStatus;
  error: unknown;
}) {
  return (
    <div className="panel">
      <div className="panel-head">
        <span>
          <span className="status-dot live" />
          mesh
        </span>
        <span className="count">
          {status
            ? `${status.total_connected} online · ${status.total_meshes} mesh${status.total_meshes === 1 ? "" : "es"}`
            : "…"}
        </span>
      </div>
      {error ? (
        <div className="err" style={{ padding: "24px" }}>
          {String(error)}
        </div>
      ) : !status ? (
        <div className="empty">loading…</div>
      ) : status.meshes.length === 0 ? (
        <Onboarding />
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
    </div>
  );
}

function SessionsPanel({
  sessions,
  error,
  onSelect,
}: {
  sessions?: SessionMeta[];
  error: unknown;
  onSelect: (id: string) => void;
}) {
  return (
    <div className="panel">
      <div className="panel-head">
        <span>
          <span className="status-dot live" />
          sessions
        </span>
        <span className="count">{sessions?.length ?? "…"}</span>
      </div>
      {error ? (
        <div className="err" style={{ padding: "24px" }}>
          {String(error)}
        </div>
      ) : !sessions ? (
        <div className="empty">loading…</div>
      ) : sessions.length === 0 ? (
        <div className="empty">
          no sessions yet — POST a vault to /api/v1/ingest to populate.
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
    </div>
  );
}

function Onboarding() {
  const brokerWss = `wss://${location.host}/api/v1/mesh/ws`;
  const steps: Array<{ title: string; cmd: string }> = [
    {
      title: "install tkr",
      cmd: "curl -fsSL https://github.com/einyx/tkr/releases/latest/download/install.sh | bash",
    },
    {
      title: "mint an invite (the owner key signs it)",
      cmd: `tkr mesh invite-mint --slug demo \\
  --broker-url ${brokerWss} \\
  --owner-key-file ~/.tkr/owner.env`,
    },
    {
      title: "share the invite URL — anyone runs:",
      cmd: "tkr mesh join <invite-url>",
    },
    {
      title: "tail messages in one terminal, send from another:",
      cmd: `tkr mesh tail demo                 # listen
tkr mesh send demo --to <addr> --recipient-pubkey <pub> 'hello'`,
    },
  ];
  return (
    <div className="onboard">
      <p className="onboard-intro">
        no peers connected yet. this broker accepts any peer with a signed
        invite. to bring up the mesh on your machine:
      </p>
      <ol className="onboard-steps">
        {steps.map((s, i) => (
          <li key={i}>
            <div className="onboard-step-title">{s.title}</div>
            <pre className="onboard-cmd">{s.cmd}</pre>
          </li>
        ))}
      </ol>
      <p className="onboard-hint">
        broker websocket: <code>{brokerWss}</code>
      </p>
    </div>
  );
}
