// Status banner that sits above the dashboard. Surfaces "is the
// thing on?" for the three opt-in features (body capture,
// server-side sandbox, audit drainer health) so operators see
// config state without having to scroll three panels.
//
// All inputs come from already-polled queries on Dashboard.tsx; this
// component is a pure transform. Render-only — no state of its own.

interface CapturedShape {
  enabled: boolean;
}
interface SandboxShape {
  enabled: boolean;
  total: number;
}
interface ReceiptShape {
  totalDropped: number;
  readyToDrain: boolean;
}

interface Props {
  captured?: CapturedShape;
  sandbox?: SandboxShape;
  receipts?: ReceiptShape;
}

type Tone = "on" | "off" | "warn";

interface Chip {
  label: string;
  value: string;
  tone: Tone;
  title: string;
}

export function StatusBanner({ captured, sandbox, receipts }: Props) {
  const chips: Chip[] = [
    chipForCapture(captured),
    chipForSandbox(sandbox),
    chipForReceipts(receipts),
  ];
  return (
    <div className="status-banner">
      <span className="status-banner-label">status</span>
      {chips.map((c) => (
        <span
          key={c.label}
          className={`status-chip status-chip-${c.tone}`}
          title={c.title}
        >
          <span className="status-chip-label">{c.label}</span>
          <span className="status-chip-value">{c.value}</span>
        </span>
      ))}
      <span className="status-banner-spacer" />
      <a
        className="status-banner-docs"
        href="https://github.com/einyx/jkr/blob/main/docs/operations.md"
        target="_blank"
        rel="noopener noreferrer"
      >
        ops docs ↗
      </a>
    </div>
  );
}

function chipForCapture(c?: CapturedShape): Chip {
  if (!c) return { label: "capture", value: "…", tone: "off", title: "loading" };
  return c.enabled
    ? {
        label: "capture",
        value: "on",
        tone: "on",
        title:
          "body capture is on (JKR_CAPTURE_BODIES=true) — scrubbed request + response bodies are stashed in the captured ring",
      }
    : {
        label: "capture",
        value: "off",
        tone: "off",
        title:
          "body capture is off — set JKR_CAPTURE_BODIES=true to start stashing scrubbed bodies",
      };
}

function chipForSandbox(s?: SandboxShape): Chip {
  if (!s) return { label: "sandbox", value: "…", tone: "off", title: "loading" };
  return s.enabled
    ? {
        label: "sandbox",
        value: "on",
        tone: "on",
        title:
          "server-side sandbox endpoint is enabled (JKR_SANDBOX_EXEC=true). CLI agent activity isn't ingested here; see ops docs",
      }
    : {
        label: "sandbox",
        value: "off",
        tone: "off",
        title:
          "server-side sandbox endpoint disabled. CLI agent-side sandbox still runs locally; flip JKR_SANDBOX_EXEC=true for the HTTP endpoint",
      };
}

function chipForReceipts(r?: ReceiptShape): Chip {
  if (!r) return { label: "receipts", value: "…", tone: "off", title: "loading" };
  if (r.totalDropped > 0) {
    return {
      label: "receipts",
      value: `${r.totalDropped} dropped`,
      tone: "warn",
      title:
        "drainer is behind — receipts dropped since last restart. point an external relayer at POST /api/v1/llm/receipts/drain",
    };
  }
  if (r.readyToDrain) {
    return {
      label: "receipts",
      value: "ready",
      tone: "warn",
      title: "receipts queue past flush threshold — drain now",
    };
  }
  return {
    label: "receipts",
    value: "healthy",
    tone: "on",
    title: "audit drain queue under threshold, no drops",
  };
}
