mod broker;

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use bytes::Bytes;
use http::header::{
    ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS,
    ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, CONNECTION,
    CONTENT_LENGTH, CONTENT_TYPE, COOKIE, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY,
    SET_COOKIE, UPGRADE, VARY,
};
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::time::sleep;

type Body = Full<Bytes>;

#[derive(Clone)]
struct AppState {
    inner: Arc<StateInner>,
}

struct StateInner {
    sessions: Mutex<HashMap<String, SessionState>>,
    next_event_id: AtomicU64,
    needs_setup: bool,
    ai_provider: String,
    db_configured: bool,
    vault: Mutex<BTreeMap<String, StoredSession>>,
    admin_password: String,
    broker: Arc<broker::BrokerState>,
    /// Pending receipts queued for batched on-chain settlement. Keyed by
    /// recipient address (lowercase 0x…). The aggregator service drains
    /// each bucket into a single MeshEscrow.claimBatch() call when either
    /// (a) the bucket reaches `AGGREGATOR_BATCH_SIZE`, or (b) the oldest
    /// receipt in the bucket is older than `AGGREGATOR_MAX_AGE_SECS`.
    aggregator: Mutex<BTreeMap<String, Vec<QueuedReceipt>>>,
    /// Upstream EVM JSON-RPC URL. When set, /api/v1/chain/rpc proxies POST
    /// bodies here. Configure via TKR_CHAIN_RPC_URL; defaults to none
    /// (the route returns 503 unless configured).
    chain_rpc_url: Option<String>,
}

const INDEX_HTML: &str = include_str!("../static/index.html");

// Wire types mirror crates/tkr-session-recorder/src/storage.rs. Kept inline
// rather than depending on the recorder so the server doesn't pull in the
// wasm-host trait surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultEvent {
    ts: String,
    session_id: String,
    seq: u64,
    tool: String,
    input: serde_json::Value,
    output_preview: String,
    #[serde(default)]
    output_full_ref: Option<String>,
    tokens_in: u32,
    tokens_out: u32,
    filter_savings_tokens: u32,
    #[serde(default)]
    cache_hit: Option<bool>,
    duration_ms: u32,
    #[serde(default)]
    exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultMeta {
    session_id: String,
    started_at: String,
    #[serde(default)]
    ended_at: Option<String>,
    agent: String,
    #[serde(default)]
    project_root: Option<String>,
    tkr_version: String,
}

#[derive(Debug, Clone, Deserialize)]
struct IngestPayload {
    meta: VaultMeta,
    #[serde(default)]
    events: Vec<VaultEvent>,
}

#[derive(Debug, Clone, Serialize)]
struct StoredSession {
    meta: VaultMeta,
    events: Vec<VaultEvent>,
}

#[derive(Clone)]
struct SessionState {
    current_tenant_id: String,
}

#[derive(Serialize)]
struct MeResponse<'a> {
    user: User<'a>,
    tenants: [Tenant<'a>; 2],
    #[serde(rename = "currentTenantId")]
    current_tenant_id: &'a str,
}

#[derive(Serialize)]
struct User<'a> {
    id: &'a str,
    email: &'a str,
    #[serde(rename = "displayName")]
    display_name: &'a str,
}

#[derive(Serialize)]
struct Tenant<'a> {
    id: &'a str,
    name: &'a str,
    role: &'a str,
}

fn main() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(4000);
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid listen address {host}:{port}"))?;

    // Refuse to start with a public-facing bind unless TKR_ADMIN_PASSWORD is
    // set. Loopback (127.0.0.1, ::1) gets a dev fallback so local development
    // continues to "just work".
    let env_password = std::env::var("TKR_ADMIN_PASSWORD").ok();
    let is_loopback = addr.ip().is_loopback();
    let admin_password = match (env_password, is_loopback) {
        (Some(p), _) if p.len() >= 8 => p,
        (Some(_), _) => {
            anyhow::bail!("TKR_ADMIN_PASSWORD must be at least 8 characters")
        }
        (None, true) => {
            eprintln!("tkr-server: TKR_ADMIN_PASSWORD unset, using dev password 'correct' (loopback only)");
            "correct".to_string()
        }
        (None, false) => {
            anyhow::bail!(
                "refusing to bind to non-loopback address {addr} without TKR_ADMIN_PASSWORD set \
                 (use HOST=127.0.0.1 for local dev, or set TKR_ADMIN_PASSWORD for public bind)"
            )
        }
    };

    let state = AppState {
        inner: Arc::new(StateInner {
            sessions: Mutex::new(HashMap::new()),
            next_event_id: AtomicU64::new(1),
            needs_setup: std::env::var("SERVICE_NEEDS_SETUP")
                .map(|v| v == "1")
                .unwrap_or(false),
            ai_provider: std::env::var("AI_PROVIDER").unwrap_or_else(|_| "openai".to_string()),
            db_configured: std::env::var("DATABASE_URL").map(|v| !v.is_empty()).unwrap_or(false),
            vault: Mutex::new(BTreeMap::new()),
            admin_password,
            broker: broker::BrokerState::new(),
            aggregator: Mutex::new(BTreeMap::new()),
            chain_rpc_url: std::env::var("TKR_CHAIN_RPC_URL").ok().filter(|s| !s.is_empty()),
        }),
    };

    let listener = TcpListener::bind(addr).await?;
    eprintln!("tkr-server listening on http://{addr}");

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let svc = service_fn(move |req| route(req, state.clone()));
            if let Err(err) = http1::Builder::new()
                .serve_connection(io, svc)
                .with_upgrades()
                .await
            {
                eprintln!("tkr-server connection error: {err}");
            }
        });
    }
}

async fn route(req: Request<Incoming>, state: AppState) -> Result<Response<Body>, Infallible> {
    let response = match (req.method(), req.uri().path()) {
        (&Method::OPTIONS, _) => no_content(&req),
        (&Method::GET, "/") | (&Method::GET, "/index.html") => html_response(INDEX_HTML),
        (&Method::GET, "/health") => json_response(
            req.headers(),
            StatusCode::OK,
            json!({
                "ok": true,
                "service": "tkr-server",
                "aiProvider": state.inner.ai_provider,
                "dbConfigured": state.inner.db_configured
            }),
        ),
        (&Method::GET, "/api/auth/config") => json_response(
            req.headers(),
            StatusCode::OK,
            json!({
                "mode": "self_host_multi",
                "providers": ["password"],
                "needsSetup": state.inner.needs_setup
            }),
        ),
        (&Method::POST, "/api/auth/login") => handle_login(req, state).await,
        (&Method::POST, "/api/auth/logout") => handle_logout(req, state),
        (&Method::GET, "/api/auth/me") => handle_me(&req, state),
        (&Method::POST, "/api/auth/setup") => handle_setup(req, state).await,
        (&Method::POST, "/api/auth/switch-tenant") => handle_switch_tenant(req, state).await,
        (&Method::GET, "/api/v1/stream") => handle_stream(req, state).await,
        (&Method::POST, "/api/v1/mesh/join") => handle_mesh_join(req, state).await,
        (&Method::GET, "/api/v1/mesh/ws") => handle_mesh_ws(req, state).await,
        (&Method::GET, "/api/v1/mesh/status") => json_response(
            req.headers(),
            StatusCode::OK,
            state.inner.broker.status(),
        ),
        (&Method::POST, "/api/v1/chain/rpc") => handle_chain_rpc(req, state).await,
        (&Method::POST, "/api/v1/aggregator/queue") => handle_aggregator_queue(req, state).await,
        (&Method::GET, "/api/v1/aggregator/pending") => handle_aggregator_pending(&req, state),
        (&Method::POST, "/api/v1/ingest") => handle_ingest(req, state).await,
        (&Method::GET, "/api/v1/sessions") => handle_list_sessions(&req, state),
        (&Method::GET, path)
            if path.starts_with("/api/v1/sessions/") && path.ends_with("/events") =>
        {
            let id = path
                .strip_prefix("/api/v1/sessions/")
                .and_then(|s| s.strip_suffix("/events"))
                .unwrap_or("");
            handle_get_events(&req, state, id)
        }
        _ => json_response(
            req.headers(),
            StatusCode::NOT_FOUND,
            json!({ "error": { "code": "not_found", "message": "not found" } }),
        ),
    };
    Ok(response)
}

fn html_response(body: &'static str) -> Response<Body> {
    let bytes = Bytes::from_static(body.as_bytes());
    let len = bytes.len();
    let mut builder = Response::builder().status(StatusCode::OK);
    let headers = builder.headers_mut().expect("headers");
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&len.to_string()).expect("content length"),
    );
    builder.body(Full::new(bytes)).expect("response")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn no_content(req: &Request<Incoming>) -> Response<Body> {
    let mut builder = Response::builder().status(StatusCode::NO_CONTENT);
    apply_cors_headers(builder.headers_mut().expect("headers_mut"), req.headers());
    builder.body(Full::new(Bytes::new())).expect("response")
}

async fn handle_login(req: Request<Incoming>, state: AppState) -> Response<Body> {
    let origin_headers = req.headers().clone();
    let payload: serde_json::Value = match read_json(req).await {
        Ok(value) => value,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body must be valid json",
                &origin_headers,
            )
        }
    };
    let password_ok = payload
        .get("password")
        .and_then(|v| v.as_str())
        .map(|v| constant_time_eq(v.as_bytes(), state.inner.admin_password.as_bytes()))
        .unwrap_or(false);
    if !password_ok {
        return json_error(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "wrong password",
            &origin_headers,
        );
    }

    let session_id = new_session_id();
    {
        let mut sessions = state.inner.sessions.lock().expect("sessions lock");
        sessions.insert(
            session_id.clone(),
            SessionState {
                current_tenant_id: "tenant-dev".to_string(),
            },
        );
    }

    let mut res = json_response(&origin_headers, StatusCode::OK, json!({ "ok": true }));
    res.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&format!(
            "tkr_session={session_id}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=604800"
        ))
        .expect("set-cookie"),
    );
    res
}

fn handle_logout(req: Request<Incoming>, state: AppState) -> Response<Body> {
    if let Some(session_id) = session_cookie(req.headers()) {
        let mut sessions = state.inner.sessions.lock().expect("sessions lock");
        sessions.remove(&session_id);
    }
    let mut res = json_response(req.headers(), StatusCode::OK, json!({ "ok": true }));
    res.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_static("tkr_session=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0"),
    );
    res
}

fn handle_me(req: &Request<Incoming>, state: AppState) -> Response<Body> {
    let session_id = match session_cookie(req.headers()) {
        Some(id) => id,
        None => return unauth(req),
    };
    let current_tenant = {
        let sessions = state.inner.sessions.lock().expect("sessions lock");
        match sessions.get(&session_id) {
            Some(session) => session.current_tenant_id.clone(),
            None => return unauth(req),
        }
    };

    let body = MeResponse {
        user: User {
            id: "user-dev",
            email: "dev@example.com",
            display_name: "Dev User",
        },
        tenants: [
            Tenant {
                id: "tenant-dev",
                name: "Dev Workspace",
                role: "owner",
            },
            Tenant {
                id: "tenant-prod",
                name: "Production",
                role: "admin",
            },
        ],
        current_tenant_id: &current_tenant,
    };
    json_response(req.headers(), StatusCode::OK, body)
}

async fn handle_setup(req: Request<Incoming>, state: AppState) -> Response<Body> {
    if !state.inner.needs_setup {
        return json_error(
            StatusCode::CONFLICT,
            "already_setup",
            "owner exists",
            req.headers(),
        );
    }
    handle_login(req, state).await
}

async fn handle_switch_tenant(req: Request<Incoming>, state: AppState) -> Response<Body> {
    let origin_headers = req.headers().clone();
    let session_id = match session_cookie(&origin_headers) {
        Some(id) => id,
        None => return unauth(&req),
    };
    let payload: serde_json::Value = match read_json(req).await {
        Ok(value) => value,
        Err(_) => serde_json::Value::Null,
    };
    if let Some(tenant_id) = payload.get("tenantId").and_then(|v| v.as_str()) {
        if matches!(tenant_id, "tenant-dev" | "tenant-prod") {
            let mut sessions = state.inner.sessions.lock().expect("sessions lock");
            if let Some(session) = sessions.get_mut(&session_id) {
                session.current_tenant_id = tenant_id.to_string();
            } else {
                return json_error(
                    StatusCode::UNAUTHORIZED,
                    "unauth",
                    "not logged in",
                    &origin_headers,
                );
            }
        }
    }
    json_response(&origin_headers, StatusCode::OK, json!({ "ok": true }))
}

async fn handle_stream(req: Request<Incoming>, state: AppState) -> Response<Body> {
    if session_cookie(req.headers())
        .and_then(|session_id| {
            state
                .inner
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&session_id)
                .cloned()
        })
        .is_none()
    {
        return unauth(&req);
    }

    let key = match req.headers().get(SEC_WEBSOCKET_KEY) {
        Some(key) => key.clone(),
        None => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "missing_websocket_key",
                "missing websocket key",
                req.headers(),
            )
        }
    };

    let accept = websocket_accept(key.as_bytes());
    let state_for_task = state.clone();
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                if let Err(err) = websocket_writer(upgraded, state_for_task).await {
                    eprintln!("tkr-server stream error: {err:#}");
                }
            }
            Err(err) => eprintln!("tkr-server upgrade error: {err}"),
        }
    });

    let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    let headers = builder.headers_mut().expect("headers");
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(
        SEC_WEBSOCKET_ACCEPT,
        HeaderValue::from_str(&accept).expect("websocket accept"),
    );
    apply_cors_headers(headers, &HeaderMap::new());
    builder.body(Full::new(Bytes::new())).expect("response")
}

async fn handle_mesh_join(req: Request<Incoming>, state: AppState) -> Response<Body> {
    let origin_headers = req.headers().clone();
    if session_cookie(&origin_headers)
        .and_then(|sid| {
            state
                .inner
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&sid)
                .cloned()
        })
        .is_none()
    {
        return unauth(&req);
    }
    let payload: serde_json::Value = match read_json(req).await {
        Ok(v) => v,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body must be valid json",
                &origin_headers,
            )
        }
    };
    let body: broker::JoinRequest = match serde_json::from_value(payload) {
        Ok(v) => v,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                &format!("expected {{invite_token, invite_payload, address, display_name?}}: {e}"),
                &origin_headers,
            )
        }
    };
    match broker::handle_join(&state.inner.broker, body, unix_ts()) {
        Ok(resp) => json_response(&origin_headers, StatusCode::OK, resp),
        Err((status, err)) => json_response(
            &origin_headers,
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
            err,
        ),
    }
}

async fn handle_mesh_ws(req: Request<Incoming>, state: AppState) -> Response<Body> {
    if session_cookie(req.headers())
        .and_then(|sid| {
            state
                .inner
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&sid)
                .cloned()
        })
        .is_none()
    {
        return unauth(&req);
    }
    // Reuse the same SHA-1 / base64 handshake we already do for /api/v1/stream.
    let key = match req.headers().get(SEC_WEBSOCKET_KEY) {
        Some(k) => k.clone(),
        None => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "missing_websocket_key",
                "missing websocket key",
                req.headers(),
            )
        }
    };
    let accept = websocket_accept(key.as_bytes());
    let broker = state.inner.broker.clone();

    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                let io = TokioIo::new(upgraded);
                let ws = tokio_tungstenite::WebSocketStream::from_raw_socket(
                    io,
                    tokio_tungstenite::tungstenite::protocol::Role::Server,
                    None,
                )
                .await;
                broker::run_ws_session(broker, ws).await;
            }
            Err(err) => eprintln!("tkr-server mesh upgrade error: {err}"),
        }
    });

    let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    let headers = builder.headers_mut().expect("headers");
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(
        SEC_WEBSOCKET_ACCEPT,
        HeaderValue::from_str(&accept).expect("websocket accept"),
    );
    apply_cors_headers(headers, &HeaderMap::new());
    builder.body(Full::new(Bytes::new())).expect("response")
}

// ---------- Aggregator (batched receipt settlement) ----------

/// Maximum receipts per recipient bucket before the aggregator service
/// should flush via MeshEscrow.claimBatch(). At ~30k gas saved per claim
/// vs a single tx, batches of 4-8 hit the gas/latency sweet spot.
const AGGREGATOR_BATCH_SIZE: usize = 8;

/// Maximum age of the oldest queued receipt before the aggregator should
/// flush even if the bucket isn't full. Keeps p99 latency bounded.
const AGGREGATOR_MAX_AGE_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueuedReceipt {
    /// 0x-prefixed bytes32 hex.
    #[serde(rename = "sessionId")]
    session_id: String,
    /// Decimal string (uint256-safe).
    cumulative: String,
    /// 0x-prefixed 65-byte signature.
    signature: String,
    /// 0x-prefixed lowercase recipient address (== msg.sender on claim).
    recipient: String,
    /// EVM chain id the receipt was signed for.
    #[serde(rename = "chainId")]
    chain_id: u64,
    /// MeshEscrow contract address the receipt is valid against.
    contract: String,
    /// Server-assigned unix-secs queue entry time. Used by the flush
    /// daemon to honor AGGREGATOR_MAX_AGE_SECS. Filled in by the handler;
    /// any client-supplied value is ignored.
    #[serde(rename = "queuedAt", default)]
    queued_at: u64,
}

async fn handle_aggregator_queue(req: Request<Incoming>, state: AppState) -> Response<Body> {
    let origin_headers = req.headers().clone();
    if session_cookie(&origin_headers)
        .and_then(|sid| {
            state
                .inner
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&sid)
                .cloned()
        })
        .is_none()
    {
        return unauth(&req);
    }
    let payload: serde_json::Value = match read_json(req).await {
        Ok(v) => v,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body must be valid json",
                &origin_headers,
            )
        }
    };
    let mut entry: QueuedReceipt = match serde_json::from_value(payload) {
        Ok(v) => v,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                &format!(
                    "expected {{sessionId, cumulative, signature, recipient, chainId, contract}}: {e}"
                ),
                &origin_headers,
            )
        }
    };
    entry.queued_at = unix_ts();
    entry.recipient = entry.recipient.to_ascii_lowercase();

    let bucket_key = entry.recipient.clone();
    let (bucket_size, ready_to_flush) = {
        let mut agg = state.inner.aggregator.lock().expect("aggregator lock");
        let bucket = agg.entry(bucket_key.clone()).or_default();
        bucket.push(entry);
        let size = bucket.len();
        let oldest = bucket.first().map(|r| r.queued_at).unwrap_or(unix_ts());
        let ready = size >= AGGREGATOR_BATCH_SIZE
            || unix_ts().saturating_sub(oldest) >= AGGREGATOR_MAX_AGE_SECS;
        (size, ready)
    };

    json_response(
        &origin_headers,
        StatusCode::OK,
        json!({
            "ok": true,
            "recipient": bucket_key,
            "bucketSize": bucket_size,
            "readyToFlush": ready_to_flush,
            "batchSize": AGGREGATOR_BATCH_SIZE,
            "maxAgeSecs": AGGREGATOR_MAX_AGE_SECS,
        }),
    )
}

fn handle_aggregator_pending(req: &Request<Incoming>, state: AppState) -> Response<Body> {
    if session_cookie(req.headers())
        .and_then(|sid| {
            state
                .inner
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&sid)
                .cloned()
        })
        .is_none()
    {
        return unauth(req);
    }
    let agg = state.inner.aggregator.lock().expect("aggregator lock");
    let buckets: Vec<serde_json::Value> = agg
        .iter()
        .map(|(recipient, receipts)| {
            json!({
                "recipient": recipient,
                "count": receipts.len(),
                "oldestQueuedAt": receipts.first().map(|r| r.queued_at),
            })
        })
        .collect();
    let total: usize = agg.values().map(|v| v.len()).sum();
    json_response(
        req.headers(),
        StatusCode::OK,
        json!({
            "totalPending": total,
            "buckets": buckets,
            "batchSize": AGGREGATOR_BATCH_SIZE,
            "maxAgeSecs": AGGREGATOR_MAX_AGE_SECS,
        }),
    )
}

async fn handle_chain_rpc(req: Request<Incoming>, state: AppState) -> Response<Body> {
    use http_body_util::BodyExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let upstream = match state.inner.chain_rpc_url.as_deref() {
        Some(u) => u,
        None => {
            return json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "chain_rpc_unconfigured",
                "TKR_CHAIN_RPC_URL is not set on this server",
                req.headers(),
            )
        }
    };

    // Parse the upstream URL into (host, port, path). Only http:// is
    // supported — the chain runs in the same compose network, no TLS.
    let stripped = match upstream.strip_prefix("http://") {
        Some(s) => s,
        None => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "chain_rpc_misconfigured",
                "TKR_CHAIN_RPC_URL must start with http://",
                req.headers(),
            )
        }
    };
    let (authority, path) = match stripped.find('/') {
        Some(i) => (&stripped[..i], &stripped[i..]),
        None => (stripped, "/"),
    };
    let (host, port): (&str, u16) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(8545)),
        None => (authority, 80),
    };

    // Read the body up-front (RPC bodies are small; we cap at 256 KiB
    // defensively even though the standard read_json caps further).
    let origin_headers = req.headers().clone();
    let body_bytes = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "body_read_failed",
                "could not read request body",
                &origin_headers,
            )
        }
    };
    if body_bytes.len() > 256 * 1024 {
        return json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "RPC body exceeds 256 KiB",
            &origin_headers,
        );
    }

    // Build the upstream HTTP request manually. Tight loop: connect, write,
    // read until upstream closes (HTTP/1.0) or until we've consumed
    // Content-Length (HTTP/1.1).
    let upstream_req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        len = body_bytes.len()
    );
    let connect = TcpStream::connect((host, port));
    let mut sock = match tokio::time::timeout(Duration::from_secs(5), connect).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return json_error(
                StatusCode::BAD_GATEWAY,
                "upstream_connect",
                &format!("connect {host}:{port}: {e}"),
                &origin_headers,
            )
        }
        Err(_) => {
            return json_error(
                StatusCode::GATEWAY_TIMEOUT,
                "upstream_connect_timeout",
                "timed out connecting to upstream chain",
                &origin_headers,
            )
        }
    };
    if sock.write_all(upstream_req.as_bytes()).await.is_err()
        || sock.write_all(&body_bytes).await.is_err()
    {
        return json_error(
            StatusCode::BAD_GATEWAY,
            "upstream_write",
            "failed to send request to upstream",
            &origin_headers,
        );
    }

    // Read until EOF (we requested Connection: close); cap at 16 MiB.
    let mut raw = Vec::with_capacity(4096);
    let mut buf = [0u8; 16 * 1024];
    let read_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = read_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, sock.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.len() > 16 * 1024 * 1024 {
                    return json_error(
                        StatusCode::BAD_GATEWAY,
                        "upstream_too_large",
                        "upstream response exceeded 16 MiB",
                        &origin_headers,
                    );
                }
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }

    // Split headers / body at "\r\n\r\n" and find Content-Type / status.
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n");
    let (head, body) = match split {
        Some(i) => (&raw[..i], &raw[i + 4..]),
        None => (raw.as_slice(), &[][..]),
    };
    let head_str = std::str::from_utf8(head).unwrap_or("");
    let mut lines = head_str.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(502);

    // Build our response with the upstream body verbatim. Force JSON content
    // type — anvil sends application/json which is what callers expect.
    let bytes = Bytes::copy_from_slice(body);
    let mut builder = Response::builder().status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
    let headers = builder.headers_mut().expect("headers");
    apply_cors_headers(headers, &origin_headers);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&bytes.len().to_string()).expect("len"),
    );
    builder.body(Full::new(bytes)).expect("response")
}

async fn handle_ingest(req: Request<Incoming>, state: AppState) -> Response<Body> {
    let origin_headers = req.headers().clone();
    if session_cookie(&origin_headers)
        .and_then(|sid| {
            state
                .inner
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&sid)
                .cloned()
        })
        .is_none()
    {
        return unauth(&req);
    }

    let payload = match read_json(req).await {
        Ok(value) => value,
        Err(_) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body must be valid json",
                &origin_headers,
            )
        }
    };
    let ingest: IngestPayload = match serde_json::from_value(payload) {
        Ok(v) => v,
        Err(e) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "invalid_payload",
                &format!("expected {{meta, events}}: {e}"),
                &origin_headers,
            )
        }
    };

    if ingest.meta.session_id.is_empty() {
        return json_error(
            StatusCode::BAD_REQUEST,
            "invalid_session_id",
            "meta.session_id is required",
            &origin_headers,
        );
    }
    if let Some(bad) = ingest
        .events
        .iter()
        .find(|e| e.session_id != ingest.meta.session_id)
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            "session_id_mismatch",
            &format!(
                "event session_id {:?} does not match meta {:?}",
                bad.session_id, ingest.meta.session_id
            ),
            &origin_headers,
        );
    }

    let session_id = ingest.meta.session_id.clone();
    let event_count = ingest.events.len();
    let mut events = ingest.events;
    events.sort_by_key(|e| e.seq);

    let mut vault = state.inner.vault.lock().expect("vault lock");
    vault.insert(
        session_id.clone(),
        StoredSession {
            meta: ingest.meta,
            events,
        },
    );

    json_response(
        &origin_headers,
        StatusCode::OK,
        json!({
            "ok": true,
            "sessionId": session_id,
            "events": event_count,
        }),
    )
}

fn handle_list_sessions(req: &Request<Incoming>, state: AppState) -> Response<Body> {
    if session_cookie(req.headers())
        .and_then(|sid| {
            state
                .inner
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&sid)
                .cloned()
        })
        .is_none()
    {
        return unauth(req);
    }

    let vault = state.inner.vault.lock().expect("vault lock");
    let metas: Vec<&VaultMeta> = vault.values().map(|s| &s.meta).collect();
    json_response(req.headers(), StatusCode::OK, json!({ "sessions": metas }))
}

fn handle_get_events(req: &Request<Incoming>, state: AppState, id: &str) -> Response<Body> {
    if session_cookie(req.headers())
        .and_then(|sid| {
            state
                .inner
                .sessions
                .lock()
                .expect("sessions lock")
                .get(&sid)
                .cloned()
        })
        .is_none()
    {
        return unauth(req);
    }

    let vault = state.inner.vault.lock().expect("vault lock");
    match vault.get(id) {
        Some(stored) => json_response(
            req.headers(),
            StatusCode::OK,
            json!({ "meta": &stored.meta, "events": &stored.events }),
        ),
        None => json_error(
            StatusCode::NOT_FOUND,
            "session_not_found",
            "no session with that id",
            req.headers(),
        ),
    }
}

async fn websocket_writer(upgraded: Upgraded, state: AppState) -> anyhow::Result<()> {
    let mut io = TokioIo::new(upgraded);
    write_event_triplet(&mut io, &state).await?;
    loop {
        sleep(Duration::from_millis(2_500)).await;
        write_event_triplet(&mut io, &state).await?;
    }
}

async fn write_event_triplet(io: &mut TokioIo<Upgraded>, state: &AppState) -> anyhow::Result<()> {
    let cmds = [
        "git status",
        "cargo test",
        "pnpm install",
        "git diff",
        "docker compose ps",
    ];
    let idx = (state.inner.next_event_id.load(Ordering::Relaxed) as usize) % cmds.len();
    let cmd = cmds[idx];
    let now = unix_ts();
    let start = json!({
        "id": next_event_id(state),
        "ts": now,
        "type": "command_start",
        "cmd": cmd,
    });
    let line = json!({
        "id": next_event_id(state),
        "ts": now,
        "type": "line",
        "line": format!("> {cmd}"),
    });
    let end = json!({
        "id": next_event_id(state),
        "ts": now,
        "type": "command_end",
        "cmd": cmd,
        "tokensIn": 1800,
        "tokensSaved": 800 + (next_event_id(state) % 3200),
    });

    for event in [start, line, end] {
        let payload = serde_json::to_vec(&event)?;
        let frame = websocket_text_frame(&payload);
        io.write_all(&frame).await?;
    }
    io.flush().await?;
    Ok(())
}

fn next_event_id(state: &AppState) -> u64 {
    state.inner.next_event_id.fetch_add(1, Ordering::Relaxed)
}

fn unix_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn websocket_text_frame(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 4);
    frame.push(0x81);
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

fn websocket_accept(key: &[u8]) -> String {
    let mut src = Vec::with_capacity(key.len() + 36);
    src.extend_from_slice(key);
    src.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let digest = sha1_digest(&src);
    BASE64.encode(digest)
}

fn sha1_digest(input: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xEFCD_AB89;
    let mut h2: u32 = 0x98BA_DCFE;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xC3D2_E1F0;

    let bit_len = (input.len() as u64) * 8;
    let mut data = input.to_vec();
    data.push(0x80);
    while (data.len() % 64) != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let base = i * 4;
            *word = u32::from_be_bytes([
                chunk[base],
                chunk[base + 1],
                chunk[base + 2],
                chunk[base + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => (((b & c) | ((!b) & d)), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => (((b & c) | (b & d) | (c & d)), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

/// Hard cap on JSON request bodies. Anything larger is rejected without
/// reading it into memory — protects against trivial OOM via oversized
/// ingest/join payloads. Tune via TKR_MAX_BODY_BYTES if needed.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

async fn read_json(req: Request<Incoming>) -> anyhow::Result<serde_json::Value> {
    use http_body_util::{BodyExt, Limited};
    let cap = std::env::var("TKR_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(MAX_BODY_BYTES);
    if let Some(len) = req
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
    {
        if len > cap {
            anyhow::bail!("request body too large: {len} > {cap}");
        }
    }
    let limited = Limited::new(req.into_body(), cap);
    let bytes = limited
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("body read failed (cap {cap} bytes): {e}"))?
        .to_bytes();
    if bytes.is_empty() {
        return Ok(serde_json::Value::Object(Default::default()));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn json_response<T: Serialize>(headers_src: &HeaderMap, status: StatusCode, body: T) -> Response<Body> {
    let payload = serde_json::to_vec(&body).expect("json payload");
    let mut builder = Response::builder().status(status);
    let headers = builder.headers_mut().expect("headers");
    apply_cors_headers(headers, headers_src);
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&payload.len().to_string()).expect("content length"),
    );
    builder
        .body(Full::new(Bytes::from(payload)))
        .expect("response")
}

fn json_error(
    status: StatusCode,
    code: &str,
    message: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    let body = json!({ "error": { "code": code, "message": message } });
    json_response(headers, status, body)
}

fn unauth(req: &Request<Incoming>) -> Response<Body> {
    json_response(
        req.headers(),
        StatusCode::UNAUTHORIZED,
        json!({ "error": { "code": "unauth", "message": "not logged in" } }),
    )
}

fn apply_cors_headers(dst: &mut HeaderMap, src: &HeaderMap) {
    // Allowlist of origins permitted to make credentialed requests. Reflecting
    // arbitrary origins with `Access-Control-Allow-Credentials: true` is
    // equivalent to wildcard-with-credentials and lets any page on any domain
    // call this API as the logged-in user.
    let origin = src.get("origin").and_then(|value| value.to_str().ok());
    let allowed: &[&str] = &[
        "http://localhost:3001",
        "http://127.0.0.1:3001",
        "http://localhost:4000",
        "http://127.0.0.1:4000",
        "https://tkr.prysm.sh",
    ];
    let extra_allowed = std::env::var("TKR_ALLOWED_ORIGIN").ok();
    if let Some(o) = origin {
        let permitted =
            allowed.iter().any(|a| *a == o) || extra_allowed.as_deref() == Some(o);
        if permitted {
            if let Ok(hv) = HeaderValue::from_str(o) {
                dst.insert(ACCESS_CONTROL_ALLOW_ORIGIN, hv);
                dst.insert(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
            }
        }
    }
    dst.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET,POST,OPTIONS"),
    );
    dst.insert(ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("content-type"));
    dst.insert(VARY, HeaderValue::from_static("Origin"));
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(';').find_map(|part| {
                let trimmed = part.trim();
                trimmed
                    .strip_prefix("tkr_session=")
                    .map(|session| session.to_string())
            })
        })
}

fn new_session_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}
