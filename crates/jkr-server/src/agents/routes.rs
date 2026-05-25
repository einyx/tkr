//! HTTP routes for the agent + session subsystem. Owner-scoped reads
//! and writes resolved from either a dashboard session cookie or an
//! `x-jkr-token` header (the CLI path). Every handler returns a 503 when
//! the subsystem is disabled (no Postgres or no Docker at boot).

use bytes::Bytes;
use http::{HeaderMap, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::Request;
use jkr_api::AgentEventKind;
use serde_json::json;

use crate::{
    cli_token_lookup, json_error, json_response, require_session, AppState, Body, ChannelBody,
};

/// Resolve caller owner from a session cookie OR an x-jkr-token header.
pub async fn resolve_owner(state: &AppState, headers: &HeaderMap) -> Option<String> {
    if let Some(s) = require_session(state, headers).await {
        return Some(s.user_id);
    }
    let tok = headers.get("x-jkr-token")?.to_str().ok()?;
    if tok.is_empty() {
        return None;
    }
    cli_token_lookup(state, tok).await
}

fn unauthorized(headers: &HeaderMap) -> Response<Body> {
    json_error(
        StatusCode::UNAUTHORIZED,
        "unauthenticated",
        "log in or present a valid x-jkr-token",
        headers,
    )
}

fn unavailable(headers: &HeaderMap) -> Response<Body> {
    json_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "agent_sessions_unavailable",
        "agent sessions unavailable: requires Postgres + Docker",
        headers,
    )
}

/// Read the full request body as a UTF-8 string. Used for manifest TOML
/// uploads. Errors map to a 400.
async fn read_body_string(req: Request<Incoming>) -> Result<String, Response<Body>> {
    let headers = req.headers().clone();
    let bytes = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return Err(json_error(
                StatusCode::BAD_REQUEST,
                "body_read_failed",
                "could not read request body",
                &headers,
            ))
        }
    };
    String::from_utf8(bytes.to_vec()).map_err(|_| {
        json_error(
            StatusCode::BAD_REQUEST,
            "invalid_utf8",
            "request body must be UTF-8",
            &headers,
        )
    })
}

async fn read_body_json(req: Request<Incoming>) -> Result<serde_json::Value, Response<Body>> {
    let headers = req.headers().clone();
    let bytes = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return Err(json_error(
                StatusCode::BAD_REQUEST,
                "body_read_failed",
                "could not read request body",
                &headers,
            ))
        }
    };
    if bytes.is_empty() {
        return Ok(serde_json::Value::Object(Default::default()));
    }
    serde_json::from_slice(&bytes).map_err(|e| {
        json_error(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            &format!("request body must be valid JSON: {e}"),
            &headers,
        )
    })
}

// ── agent registry routes ──────────────────────────────────────────────────

pub async fn handle_agents_push(req: Request<Incoming>, state: AppState) -> Response<Body> {
    let headers = req.headers().clone();
    let owner = match resolve_owner(&state, &headers).await {
        Some(o) => o,
        None => return unauthorized(&headers),
    };
    let registry = match state.inner.agent_registry.as_ref() {
        Some(r) => r.clone(),
        None => return unavailable(&headers),
    };
    let toml = match read_body_string(req).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    match registry.push(&owner, &toml).await {
        Ok(version) => json_response(&headers, StatusCode::OK, json!({ "version": version })),
        Err(e) => json_error(
            StatusCode::BAD_REQUEST,
            "push_failed",
            &e.to_string(),
            &headers,
        ),
    }
}

pub async fn handle_agents_list(req: &Request<Incoming>, state: AppState) -> Response<Body> {
    let headers = req.headers();
    let owner = match resolve_owner(&state, headers).await {
        Some(o) => o,
        None => return unauthorized(headers),
    };
    let registry = match state.inner.agent_registry.as_ref() {
        Some(r) => r,
        None => return unavailable(headers),
    };
    match registry.list(&owner).await {
        Ok(rows) => {
            let agents: Vec<_> = rows
                .into_iter()
                .map(|(name, version)| json!({ "name": name, "version": version }))
                .collect();
            json_response(headers, StatusCode::OK, json!({ "agents": agents }))
        }
        Err(e) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_failed",
            &e.to_string(),
            headers,
        ),
    }
}

pub async fn handle_agent_get(
    req: &Request<Incoming>,
    state: AppState,
    name: &str,
) -> Response<Body> {
    let headers = req.headers();
    let owner = match resolve_owner(&state, headers).await {
        Some(o) => o,
        None => return unauthorized(headers),
    };
    let registry = match state.inner.agent_registry.as_ref() {
        Some(r) => r,
        None => return unavailable(headers),
    };
    let version = req.uri().query().and_then(|q| {
        url_query_lookup(q, "version").and_then(|v| v.parse::<i64>().ok())
    });
    match registry.load(&owner, name, version).await {
        Ok((ver, manifest)) => json_response(
            headers,
            StatusCode::OK,
            json!({ "name": manifest.name, "version": ver }),
        ),
        Err(e) => json_error(
            StatusCode::NOT_FOUND,
            "agent_not_found",
            &e.to_string(),
            headers,
        ),
    }
}

/// Minimal `key=value&…` query lookup (no urlencoding decode beyond `+`).
fn url_query_lookup<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

// ── session routes ──────────────────────────────────────────────────────────

pub async fn handle_session_create(req: Request<Incoming>, state: AppState) -> Response<Body> {
    let headers = req.headers().clone();
    let owner = match resolve_owner(&state, &headers).await {
        Some(o) => o,
        None => return unauthorized(&headers),
    };
    let manager = match state.inner.session_manager.as_ref() {
        Some(m) => m.clone(),
        None => return unavailable(&headers),
    };
    let body = match read_body_json(req).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let agent = match body.get("agent").and_then(|v| v.as_str()) {
        Some(a) if !a.is_empty() => a.to_string(),
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "missing_agent",
                "body must include a non-empty \"agent\" field",
                &headers,
            )
        }
    };
    let version = body.get("version").and_then(|v| v.as_i64());
    let input = body
        .get("input")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match manager.create(&owner, &agent, version, &input).await {
        Ok(sid) => json_response(&headers, StatusCode::OK, json!({ "session_id": sid })),
        Err(e) => json_error(
            StatusCode::BAD_REQUEST,
            "create_failed",
            &e.to_string(),
            &headers,
        ),
    }
}

pub async fn handle_sessions_list(req: &Request<Incoming>, state: AppState) -> Response<Body> {
    let headers = req.headers();
    let owner = match resolve_owner(&state, headers).await {
        Some(o) => o,
        None => return unauthorized(headers),
    };
    let manager = match state.inner.session_manager.as_ref() {
        Some(m) => m,
        None => return unavailable(headers),
    };
    match manager.list_sessions(&owner).await {
        Ok(rows) => {
            let sessions: Vec<_> = rows
                .into_iter()
                .map(|(id, agent, state)| {
                    json!({ "id": id, "agent": agent, "state": state })
                })
                .collect();
            json_response(headers, StatusCode::OK, json!({ "sessions": sessions }))
        }
        Err(e) => json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_failed",
            &e.to_string(),
            headers,
        ),
    }
}

pub async fn handle_session_get(
    req: &Request<Incoming>,
    state: AppState,
    id: &str,
) -> Response<Body> {
    let headers = req.headers();
    let owner = match resolve_owner(&state, headers).await {
        Some(o) => o,
        None => return unauthorized(headers),
    };
    let manager = match state.inner.session_manager.as_ref() {
        Some(m) => m,
        None => return unavailable(headers),
    };
    match manager.status_of(&owner, id).await {
        Ok(st) => json_response(headers, StatusCode::OK, json!({ "id": id, "state": st })),
        Err(e) => json_error(
            StatusCode::NOT_FOUND,
            "session_not_found",
            &e.to_string(),
            headers,
        ),
    }
}

pub async fn handle_session_stop(
    req: &Request<Incoming>,
    state: AppState,
    id: &str,
) -> Response<Body> {
    let headers = req.headers();
    let owner = match resolve_owner(&state, headers).await {
        Some(o) => o,
        None => return unauthorized(headers),
    };
    let manager = match state.inner.session_manager.as_ref() {
        Some(m) => m,
        None => return unavailable(headers),
    };
    match manager.stop_session(&owner, id).await {
        Ok(()) => json_response(headers, StatusCode::OK, json!({ "stopped": true })),
        Err(e) => json_error(
            StatusCode::NOT_FOUND,
            "session_not_found",
            &e.to_string(),
            headers,
        ),
    }
}

/// SSE stream of session events: replay persisted events from
/// `last-event-id`+1 (or 0), then follow by polling. Each event is sent
/// as `id: <seq>\ndata: <ndjson>\n\n`. The stream ends when a Done or
/// Error event is observed, or when the client disconnects.
pub async fn handle_session_stream(
    req: &Request<Incoming>,
    state: AppState,
    id: &str,
) -> Response<Body> {
    let headers = req.headers().clone();
    let owner = match resolve_owner(&state, &headers).await {
        Some(o) => o,
        None => return unauthorized(&headers),
    };
    let manager = match state.inner.session_manager.as_ref() {
        Some(m) => m.clone(),
        None => return unavailable(&headers),
    };

    // Verify ownership before opening the stream so a cross-owner id gets
    // a clean 404 rather than an empty event-stream.
    if let Err(e) = manager.status_of(&owner, id).await {
        return json_error(
            StatusCode::NOT_FOUND,
            "session_not_found",
            &e.to_string(),
            &headers,
        );
    }

    let start = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .map(|n| n + 1)
        .unwrap_or(0);

    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(32);
    let id = id.to_string();
    tokio::spawn(async move {
        let mut next = start;
        loop {
            match manager.events_since(&owner, &id, next).await {
                Ok(events) => {
                    for ev in events {
                        let frame = format!("id: {}\ndata: {}\n\n", ev.seq, ev.to_ndjson());
                        if tx.send(Bytes::from(frame)).await.is_err() {
                            return; // client gone
                        }
                        next = ev.seq as i64 + 1;
                        if matches!(
                            ev.kind,
                            AgentEventKind::Done { .. } | AgentEventKind::Error { .. }
                        ) {
                            return;
                        }
                    }
                }
                Err(_) => return,
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    });

    let body = ChannelBody { rx }.boxed();
    let mut builder = Response::builder().status(StatusCode::OK);
    let resp_headers = builder.headers_mut().expect("headers");
    resp_headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/event-stream"),
    );
    resp_headers.insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-cache"),
    );
    builder.body(body).expect("response")
}

#[cfg(test)]
mod tests {
    use super::url_query_lookup;

    #[test]
    fn query_lookup_finds_and_misses() {
        assert_eq!(url_query_lookup("version=3", "version"), Some("3"));
        assert_eq!(url_query_lookup("a=1&version=7&b=2", "version"), Some("7"));
        assert_eq!(url_query_lookup("a=1&b=2", "version"), None);
        assert_eq!(url_query_lookup("", "version"), None);
    }
}
