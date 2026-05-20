// "What gets signed?" tool. Paste a receipt JSON; this component
// reconstructs the canonical message (the exact bytes the server
// signs) so anyone building an offline verifier can confirm their
// implementation matches.
//
// No crypto runs here — the panel only constructs the message that
// would be passed to a secp256k1 ECDSA verify call. Verification
// itself happens in whatever tool the operator pipes the output
// into (k256, OpenSSL with appropriate config, etc.).
//
// Format is locked by `signer_canonical_message_is_stable` in
// crates/tkr-server/src/main.rs. If that test changes, this code
// must too — and so must docs/operations.md.

import { useState } from "react";
import { Panel } from "../Panel";

interface ReceiptShape {
  ts?: number;
  provider?: string;
  model?: string;
  status?: number;
  input_tokens?: number;
  output_tokens?: number;
  duration_ms?: number;
  sig_version?: number;
  signature?: string;
  signer_pubkey?: string;
}

const EXAMPLE: ReceiptShape = {
  ts: 1700000000,
  provider: "anthropic",
  model: "claude-sonnet-4-6",
  status: 200,
  input_tokens: 12,
  output_tokens: 7,
  duration_ms: 145,
};

export function ReceiptVerifyPanel() {
  const [raw, setRaw] = useState<string>(JSON.stringify(EXAMPLE, null, 2));
  const [output, setOutput] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const onCanon = () => {
    setErr(null);
    setCopied(false);
    try {
      const parsed = JSON.parse(raw) as ReceiptShape;
      const msg = canonicalMessage(parsed);
      setOutput(msg);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      setOutput(null);
    }
  };

  const onCopy = async () => {
    if (!output) return;
    try {
      await navigator.clipboard.writeText(output);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Swallow — clipboard may be denied; user can select+copy manually.
    }
  };

  return (
    <Panel
      title="verify a receipt"
      dot="off"
      count="canonical message tool"
      countClass="muted"
    >
      <div className="verify-body">
        <p className="verify-help">
          Paste a receipt JSON (from <code>/api/v1/llm/recent</code> or a{" "}
          <code>/drain</code> response). This tool reconstructs the exact
          UTF-8 bytes the server signs — feed those + the receipt&apos;s{" "}
          <code>signature</code> and <code>signer_pubkey</code> into any
          secp256k1 ECDSA verify and you&apos;re done. The format is{" "}
          <code>v1</code>; see <a
            href="https://github.com/einyx/tkr/blob/main/docs/operations.md#receipt-signature-protocol"
            target="_blank"
            rel="noopener noreferrer"
          >ops docs</a>.
        </p>

        <div className="verify-label">receipt JSON</div>
        <textarea
          className="verify-input"
          value={raw}
          onChange={(e) => setRaw(e.target.value)}
          spellCheck={false}
          rows={10}
        />

        <div className="verify-actions">
          <button type="button" className="sandbox-smoke-btn" onClick={onCanon}>
            canonicalize
          </button>
          {output && (
            <button
              type="button"
              className="sandbox-smoke-btn"
              onClick={onCopy}
              title="copy the canonical bytes to clipboard"
            >
              {copied ? "copied" : "copy bytes"}
            </button>
          )}
          {err && <span className="sandbox-smoke-out err-inline">{err}</span>}
        </div>

        {output && (
          <>
            <div className="verify-label">canonical message bytes</div>
            <pre className="verify-output">{output}</pre>
          </>
        )}
      </div>
    </Panel>
  );
}

/// Reconstruct the canonical signed message. MUST match
/// `ReceiptSigner::canonical_message` server-side exactly — newlines
/// only, no trailing newline, `v1\n` prefix.
function canonicalMessage(r: ReceiptShape): string {
  const fields: Array<[string, string]> = [
    ["ts", String(r.ts ?? 0)],
    ["provider", r.provider ?? ""],
    ["model", r.model ?? ""],
    ["status", String(r.status ?? 0)],
    ["input_tokens", String(r.input_tokens ?? 0)],
    ["output_tokens", String(r.output_tokens ?? 0)],
    ["duration_ms", String(r.duration_ms ?? 0)],
  ];
  const lines = fields.map(([k, v]) => `${k}=${v}`);
  return ["v1", ...lines].join("\n");
}
