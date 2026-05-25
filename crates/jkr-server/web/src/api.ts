// Thin fetch wrapper — keeps credentials, normalizes JSON errors so callers
// can just `try/catch` with a typed Error.

export class ApiError extends Error {
  status: number;
  code: string;
  constructor(status: number, code: string, message: string) {
    super(`${code}: ${message}`);
    this.status = status;
    this.code = code;
  }
}

export async function api<T = unknown>(
  path: string,
  opts: RequestInit = {},
): Promise<T> {
  const res = await fetch(path, {
    credentials: "include",
    headers: { "content-type": "application/json", ...(opts.headers || {}) },
    ...opts,
  });
  const text = await res.text();
  let body: any = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = { raw: text };
  }
  if (!res.ok) {
    const code = body?.error?.code ?? String(res.status);
    const msg = body?.error?.message ?? res.statusText;
    throw new ApiError(res.status, String(code), msg);
  }
  return body as T;
}

// ---------- domain types (mirror the server's JSON shapes) ----------

export interface Me {
  user: { id: string; email: string; displayName: string };
  tenants: Array<{ id: string; name: string; role: string }>;
  currentTenantId: string;
}

export interface MeshStatus {
  total_meshes: number;
  total_connected: number;
  meshes: Array<{ meshId: string; enrolled: number; connected: number }>;
}

export interface SessionMeta {
  session_id: string;
  agent: string;
  started_at: string;
  jkr_version: string;
}

export interface SessionEvent {
  ts: string;
  session_id: string;
  seq: number;
  tool: string;
  input: unknown;
  output_preview: string;
  tokens_in: number;
  tokens_out: number;
  filter_savings_tokens: number;
  duration_ms: number;
  exit_code?: number;
}

// Mirror of jkr-server::LlmCallReceipt. Populated by every call that
// flows through the /v1/messages Anthropic-wire proxy (and, future,
// the OpenAI ingress). Surfaced on the landing's "live gateway" panel.
export interface LlmCallReceipt {
  ts: number;          // unix seconds
  provider: string;    // "anthropic" today
  model: string;
  status: number;      // upstream HTTP status
  input_tokens: number;
  output_tokens: number;
  duration_ms: number;
}

export interface LlmRecentResponse {
  entries: LlmCallReceipt[];
  capacity: number;
}

// /api/v1/filter/stats — pre-flight redaction + injection hit counts.
// Redactions rewrite content; injections only log (or, opt-in, block).
// `total` is the redaction total kept for backward compatibility.
export interface FilterStats {
  redactions: Record<string, number>;
  total: number;
  injections: Record<string, number>;
  injections_total: number;
  injections_blocked: number;
}

// /api/v1/sandbox/recent — newest-first ring of finished runs.
// Server-capped at 64 entries; reducers run on bounded input.
export interface SandboxRunRecord {
  ts: number;
  command: string;
  exit: number;
  truncated: boolean;
  duration_ms: number;
}
export interface SandboxRecentResponse {
  entries: SandboxRunRecord[];
}

// /api/v1/sandbox/stats — whether the sandboxed-exec endpoint is
// enabled + counters + last-run snapshot. Counters are always
// returned (zero when disabled) so the dashboard can render the
// same shape regardless of feature flag state.
export interface SandboxStats {
  enabled: boolean;
  total: number;
  failed: number;
  denied: number;
  success_rate_pct: number | null;
  allowed_commands: string[];
  last: {
    ts: number;
    command: string;
    exit: number;
    truncated: boolean;
    duration_ms: number;
  } | null;
}

// /api/v1/llm/receipts/stats — audit drain queue depth + readiness.
// Fed by every successful LLM proxy call; drained by an external
// relayer via POST /api/v1/llm/receipts/drain.
export interface LlmReceiptStats {
  total: number;
  oldestQueuedAt: number | null;
  readyToDrain: boolean;
  batchSize: number;
  maxAgeSecs: number;
  queueCap: number;
  totalDropped: number;
}
