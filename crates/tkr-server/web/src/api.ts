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
  tkr_version: string;
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
