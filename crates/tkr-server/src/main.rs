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
            "tkr_session={session_id}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800"
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
        HeaderValue::from_static("tkr_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
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

async fn read_json(req: Request<Incoming>) -> anyhow::Result<serde_json::Value> {
    use http_body_util::BodyExt;
    let bytes = req.into_body().collect().await?.to_bytes();
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
    let origin = src
        .get("origin")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http://localhost:3001");
    dst.insert(
        ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_str(origin).expect("origin header"),
    );
    dst.insert(ACCESS_CONTROL_ALLOW_CREDENTIALS, HeaderValue::from_static("true"));
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
    let a = unix_ts();
    let b = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{a:08x}{b:024x}")
}
