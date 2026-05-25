// CLI tokens — Logto-gated minting of per-user bearer tokens for
// the `jkr` CLI. Replaces shared-secret JKR_INGEST_TOKEN env vars on
// user laptops: operators sign in to the dashboard once, mint a
// token here, and `jkr login` stores it in their OS keychain.
//
// The freshly-minted token is shown ONCE — it never leaves the
// server in hash-only list responses, so this panel must surface it
// inline immediately after creation. We render it inside a
// monospace box with a copy button + an explicit "I've saved it"
// dismiss action so the operator can't accidentally lose it.

import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api";
import { Panel } from "../Panel";
import { fmtRelative } from "../../lib/format";

interface TokenSummary {
  id: number;
  prefix: string;
  label: string;
  created_at: number;
  last_used_at: number;
}

interface MintResponse {
  token: string;
  label: string;
  created_at: number;
}

export function CliTokensPanel() {
  const qc = useQueryClient();
  const [label, setLabel] = useState("");
  const [justMinted, setJustMinted] = useState<MintResponse | null>(null);

  const { data, isLoading } = useQuery<{ tokens: TokenSummary[] }>({
    queryKey: ["cli-tokens"],
    queryFn: () => api("/api/v1/auth/cli-tokens"),
    refetchInterval: 10_000,
  });

  const mint = useMutation<MintResponse, Error, string>({
    mutationFn: (lbl) =>
      api<MintResponse>("/api/v1/auth/cli-token", {
        method: "POST",
        body: JSON.stringify({ label: lbl }),
      }),
    onSuccess: (resp) => {
      setJustMinted(resp);
      setLabel("");
      qc.invalidateQueries({ queryKey: ["cli-tokens"] });
    },
  });

  const revoke = useMutation<void, Error, number>({
    mutationFn: (id) =>
      api<void>(`/api/v1/auth/cli-tokens?id=${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["cli-tokens"] }),
  });

  const tokens = data?.tokens ?? [];

  return (
    <Panel
      title="CLI tokens · for `jkr login`"
      dot="live"
      count={`${tokens.length} active`}
    >
      <div style={{ padding: 12, display: "flex", flexDirection: "column", gap: 10 }}>
        <p className="muted" style={{ margin: 0, fontSize: 12, lineHeight: 1.5 }}>
          Mint a per-user bearer token for the <code>jkr</code> CLI. Store with{" "}
          <code>jkr login --url https://tkr.prysm.sh --token &lt;value&gt;</code>{" "}
          or paste interactively. Each token shown only once.
        </p>

        <div style={{ display: "flex", gap: 8, alignItems: "stretch" }}>
          <input
            type="text"
            placeholder="label (e.g. laptop, ci-runner)"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            className="verify-input"
            style={{ flex: 1, padding: "6px 10px", fontSize: 12 }}
            maxLength={64}
          />
          <button
            type="button"
            className="sandbox-smoke-btn"
            onClick={() => mint.mutate(label.trim())}
            disabled={mint.isPending}
          >
            {mint.isPending ? "minting…" : "mint token"}
          </button>
        </div>

        {mint.error && (
          <div className="err" style={{ fontSize: 12 }}>
            {mint.error.message}
          </div>
        )}

        {justMinted && (
          <div
            style={{
              border: "1px solid var(--border)",
              borderRadius: 6,
              padding: 10,
              background: "var(--bg-elev)",
              fontSize: 12,
            }}
          >
            <div className="muted" style={{ marginBottom: 6 }}>
              copy this now — it will not be shown again
            </div>
            <pre
              style={{
                margin: 0,
                padding: 8,
                background: "var(--bg)",
                borderRadius: 4,
                wordBreak: "break-all",
                whiteSpace: "pre-wrap",
                fontSize: 11,
              }}
            >
              {justMinted.token}
            </pre>
            <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
              <button
                type="button"
                className="sandbox-smoke-btn"
                onClick={() => navigator.clipboard?.writeText(justMinted.token)}
              >
                copy
              </button>
              <button
                type="button"
                className="sandbox-smoke-btn"
                onClick={() => setJustMinted(null)}
              >
                I've saved it
              </button>
            </div>
          </div>
        )}

        {isLoading ? (
          <div className="empty">loading…</div>
        ) : tokens.length === 0 ? (
          <div className="empty" style={{ fontSize: 12 }}>
            no tokens yet — mint one above.
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            {tokens.map((t) => (
              <div
                key={t.id}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 10,
                  padding: "6px 10px",
                  border: "1px solid var(--border)",
                  borderRadius: 4,
                  fontSize: 12,
                }}
              >
                <code style={{ flex: "0 0 110px" }}>{t.prefix}…</code>
                <span style={{ flex: 1 }}>{t.label || <em className="muted">no label</em>}</span>
                <span className="muted">
                  {t.last_used_at > 0
                    ? `used ${fmtRelative(t.last_used_at)}`
                    : "never used"}
                </span>
                <button
                  type="button"
                  className="sandbox-smoke-btn"
                  onClick={() => {
                    if (confirm(`revoke token ${t.prefix}…?`)) revoke.mutate(t.id);
                  }}
                  disabled={revoke.isPending}
                >
                  revoke
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </Panel>
  );
}
