import { useState, type FormEvent } from "react";
import { api, ApiError } from "../api";

interface Props {
  onSignedIn: () => void;
  onCancel: () => void;
}

export function LoginView({ onSignedIn, onCancel }: Props) {
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      await api("/api/auth/login", {
        method: "POST",
        body: JSON.stringify({ password }),
      });
      onSignedIn();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : String(err));
      setSubmitting(false);
    }
  }

  return (
    <main>
      <div className="login-card">
        <h1>tkr</h1>
        <p>session vault · sign in</p>
        <form onSubmit={handleSubmit}>
          <div className="row">
            <label>password</label>
            <input
              type="password"
              autoComplete="current-password"
              autoFocus
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </div>
          <div style={{ display: "flex", gap: "8px" }}>
            <button type="submit" disabled={submitting}>
              {submitting ? "…" : "sign in"}
            </button>
            <button
              type="button"
              onClick={onCancel}
              style={{ background: "transparent" }}
            >
              cancel
            </button>
          </div>
          {error && <div className="err">{error}</div>}
        </form>
      </div>
    </main>
  );
}
