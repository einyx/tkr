import { useQuery } from "@tanstack/react-query";
import { api, type SessionEvent, type SessionMeta } from "../api";

interface Props {
  id: string;
  onBack: () => void;
}

interface SessionDetailResponse {
  meta: SessionMeta;
  events: SessionEvent[];
}

export function SessionDetail({ id, onBack }: Props) {
  const { data, error, isLoading } = useQuery<SessionDetailResponse>({
    queryKey: ["session", id],
    queryFn: () =>
      api<SessionDetailResponse>(
        `/api/v1/sessions/${encodeURIComponent(id)}/events`,
      ),
  });

  if (error) {
    return (
      <main>
        <div className="err">{String(error)}</div>
      </main>
    );
  }
  if (isLoading || !data) {
    return (
      <main>
        <div className="empty">loading…</div>
      </main>
    );
  }

  const events = data.events ?? [];
  const totalIn = events.reduce((a, e) => a + (e.tokens_in || 0), 0);
  const totalSaved = events.reduce(
    (a, e) => a + (e.filter_savings_tokens || 0),
    0,
  );
  const pct = totalIn > 0 ? ((totalSaved / totalIn) * 100).toFixed(1) : "0.0";

  return (
    <>
      <header>
        <div className="brand">jkr</div>
        <div className="who">
          <a
            href="#"
            onClick={(e) => {
              e.preventDefault();
              onBack();
            }}
          >
            ← back
          </a>
        </div>
      </header>
      <main>
        <div className="crumbs">
          sessions / <span>{id}</span>
        </div>
        <div className="panel">
          <div className="panel-head">
            <span>session</span>
            <span className="muted">
              {data.meta?.agent ?? ""} · {data.meta?.started_at ?? ""}
            </span>
          </div>
          <table>
            <tbody>
              <tr>
                <td className="muted">tokens in</td>
                <td className="num">{totalIn.toLocaleString()}</td>
              </tr>
              <tr>
                <td className="muted">tokens saved</td>
                <td className="num ok">{totalSaved.toLocaleString()}</td>
              </tr>
              <tr>
                <td className="muted">reduction</td>
                <td className="num">{pct}%</td>
              </tr>
              <tr>
                <td className="muted">events</td>
                <td className="num">{events.length}</td>
              </tr>
            </tbody>
          </table>
        </div>
        <div className="panel">
          <div className="panel-head">
            <span>events</span>
            <span className="count">{events.length}</span>
          </div>
          {events.length === 0 ? (
            <div className="empty">no events recorded</div>
          ) : (
            <table>
              <thead>
                <tr>
                  <th>seq</th>
                  <th>tool</th>
                  <th>input</th>
                  <th className="num">in</th>
                  <th className="num">saved</th>
                  <th className="num">ms</th>
                </tr>
              </thead>
              <tbody>
                {events.map((e) => (
                  <tr key={e.seq}>
                    <td className="muted">{e.seq}</td>
                    <td>{e.tool}</td>
                    <td>
                      <pre>{JSON.stringify(e.input)}</pre>
                    </td>
                    <td className="num">
                      {(e.tokens_in || 0).toLocaleString()}
                    </td>
                    <td className="num ok">
                      {(e.filter_savings_tokens || 0).toLocaleString()}
                    </td>
                    <td className="num muted">{e.duration_ms || 0}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      </main>
    </>
  );
}
