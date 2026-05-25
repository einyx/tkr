//! Phase 6 persistence tests — exercise the real Postgres + Redis
//! paths added in the persistence refactor. Marked `#[ignore]` so
//! `cargo test` stays Docker-free by default; opt in with:
//!
//! ```bash
//! TKR_TEST_DATABASE_URL=postgres://tkr:tkr@127.0.0.1:5432/tkr_test \
//! TKR_TEST_REDIS_URL=redis://127.0.0.1:6379/1 \
//!   cargo test -p tkr-server --test persistence -- --ignored --test-threads=1
//! ```
//!
//! Tests TRUNCATE shared tables and FLUSHDB the Redis db at start, so
//! they MUST run serially (`--test-threads=1`). The easiest way to
//! satisfy that locally is to point at the compose stack with a
//! dedicated DB / Redis db number (e.g. `tkr_test` / db 1) so the
//! production schema stays untouched.
//!
//! Coverage:
//!   * `session_survives_server_restart` — Postgres-backed sessions
//!     load() across a process boundary (Phase 4 promise).
//!   * `oauth_state_survives_server_restart` — Redis OAuth state
//!     survives a restart between /auth/logto/start and the callback
//!     (Phase 3 promise). Exercised through the public endpoints, no
//!     direct Redis access from the test.
//!   * `receipts_queue_persists_across_restart` — LLM-proxy receipts
//!     written before a restart are still drainable after.
//!
//! These tests boot the real `tkr-server` binary so they exercise the
//! exact code path production runs, not a re-implementation.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;

// ───────── shared harness ─────────

struct ServerGuard {
    child: Child,
    port: u16,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pick_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = listener.local_addr().unwrap().port();
    drop(listener);
    p
}

async fn start_server(extra_env: &[(&str, String)]) -> ServerGuard {
    let port = pick_port();
    let bin = env!("CARGO_BIN_EXE_tkr-server");
    let mut cmd = Command::new(bin);
    cmd.env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        // Loopback bind avoids the public-bind admin-password check
        // and lets the dev fallback ("correct") apply.
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in extra_env {
        cmd.env(*k, v);
    }
    let child = cmd.spawn().expect("spawn tkr-server");
    for _ in 0..80 {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_millis(250)))
            .build()
            .into();
        if let Ok(resp) = agent
            .get(&format!("http://127.0.0.1:{port}/health"))
            .call()
        {
            if resp.status().as_u16() == 200 {
                return ServerGuard { child, port };
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    panic!("tkr-server failed to start on port {port}");
}

/// Pull the test DB + Redis URLs from env. Returns None when either
/// is unset; the caller skips with a printed note rather than
/// failing, so `--ignored` runs are explicit but graceful.
fn test_urls() -> Option<(String, String)> {
    let db = std::env::var("TKR_TEST_DATABASE_URL").ok()?;
    let redis_url = std::env::var("TKR_TEST_REDIS_URL").ok()?;
    Some((db, redis_url))
}

/// Wipe shared tables + Redis OAuth keys so tests start from a clean
/// slate. Cheaper than dropping/recreating the database; safe because
/// the schema lives in the production server's migration history,
/// not in this test binary.
async fn reset_state(database_url: &str, redis_url: &str) {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("connect test PG");
    // Apply migrations first — on a fresh test DB the tables don't
    // exist yet, and the server only runs migrations when it boots.
    // The `migrate!` macro bakes the SQL into this test binary at
    // compile time and is idempotent on subsequent runs.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations to test PG");
    // CASCADE handles any future FKs; TRUNCATE … RESTART IDENTITY keeps
    // the BIGSERIAL sequences from drifting across runs.
    pool.execute(
        "TRUNCATE sessions, receipts_queue, llm_recent, sandbox_recent \
         RESTART IDENTITY CASCADE",
    )
    .await
    .expect("truncate tables");
    pool.close().await;

    let client = redis::Client::open(redis_url).expect("redis client");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    let _: () = redis::cmd("FLUSHDB")
        .query_async(&mut conn)
        .await
        .expect("flushdb");
}

/// Bake the env vector the server needs. Mirrors what
/// docker-compose.yml sets. The Logto config is intentionally absent
/// so login/callback endpoints return 503 — we test the session-cookie
/// surface (`/api/auth/login` password fallback + `/api/auth/me`),
/// which doesn't need Logto.
fn server_env(database_url: &str, redis_url: &str) -> Vec<(&'static str, String)> {
    vec![
        ("DATABASE_URL", database_url.to_string()),
        ("REDIS_URL", redis_url.to_string()),
        ("TKR_ADMIN_PASSWORD", "correctbattery".to_string()),
    ]
}

// ───────── tests ─────────

/// Sign in with the password fallback, kill the server, restart it
/// with the same DATABASE_URL, and verify the same session cookie
/// still authenticates. This is the Phase 4 promise.
#[tokio::test]
#[ignore]
async fn session_survives_server_restart() {
    let (database_url, redis_url) = match test_urls() {
        Some(v) => v,
        None => {
            eprintln!("skipping: TKR_TEST_DATABASE_URL / TKR_TEST_REDIS_URL unset");
            return;
        }
    };
    reset_state(&database_url, &redis_url).await;
    let env = server_env(&database_url, &redis_url);

    // Phase A — log in, capture the session cookie.
    let cookie = {
        let guard = start_server(&env).await;
        let resp = ureq::post(&format!("http://127.0.0.1:{}/api/auth/login", guard.port))
            .header("content-type", "application/json")
            .send(r#"{"password":"correctbattery"}"#)
            .expect("login");
        assert_eq!(resp.status().as_u16(), 200, "login should succeed");
        resp.headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .expect("login should set tkr_session cookie")
            .split(';')
            .next()
            .unwrap()
            .to_string()
        // ServerGuard's Drop kills the process here — same as a real
        // restart. The in-memory HashMap dies with it.
    };
    assert!(cookie.starts_with("tkr_session="), "{cookie}");

    // Phase B — restart, re-use the cookie against /api/auth/me.
    let guard = start_server(&env).await;
    let resp = ureq::get(&format!("http://127.0.0.1:{}/api/auth/me", guard.port))
        .header("cookie", &cookie)
        .call()
        .expect("me");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "session minted before restart should still be valid after restart"
    );
}

/// `/auth/logto/start` writes an OAuth state to Redis. Kill the
/// server, restart, and verify the state is still readable by
/// inspecting Redis directly (we can't exercise the full callback
/// without a real Logto, but the storage round-trip is what Phase 3
/// guarantees). This is the inverse of the `stale_oauth_state` bug.
#[tokio::test]
#[ignore]
async fn oauth_state_survives_server_restart() {
    let (database_url, redis_url) = match test_urls() {
        Some(v) => v,
        None => {
            eprintln!("skipping: TKR_TEST_DATABASE_URL / TKR_TEST_REDIS_URL unset");
            return;
        }
    };
    reset_state(&database_url, &redis_url).await;
    let mut env = server_env(&database_url, &redis_url);
    // Need a Logto config so /auth/logto/start actually mints state
    // (otherwise it returns 503 logto_unconfigured).
    env.extend([
        ("TKR_LOGTO_ENDPOINT", "https://logto.invalid".into()),
        ("TKR_LOGTO_APP_ID", "id".into()),
        ("TKR_LOGTO_APP_SECRET", "secret".into()),
        ("TKR_LOGTO_REDIRECT_URI", "https://tkr.invalid/auth/logto/callback".into()),
    ]
    .iter()
    .map(|(k, v): &(&'static str, String)| (*k, v.clone())));

    // Phase A — fire /auth/logto/start, capture the `state` from the
    // Location header. ureq 2.x follows redirects by default, and the
    // target (Logto) is unreachable in tests, so disable redirects on
    // a dedicated agent.
    let state_param = {
        let guard = start_server(&env).await;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .into();
        let resp = agent
            .get(&format!("http://127.0.0.1:{}/auth/logto/start", guard.port))
            .call()
            .expect("logto start");
        assert_eq!(resp.status().as_u16(), 302, "should redirect to Logto");
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .expect("location header");
        // Pull state= from the query string.
        location
            .split(&['?', '&'][..])
            .find_map(|p| p.strip_prefix("state="))
            .expect("state in redirect")
            .to_string()
    };

    // Phase B — restart server, then peek at Redis directly to confirm
    // the value is still there. We can't drive the full callback
    // because we don't have a real Logto, but the storage promise is
    // what Phase 3 guarantees.
    let _guard = start_server(&env).await;
    let client = redis::Client::open(redis_url.as_str()).expect("redis client");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    let key = format!("oauth:state:{state_param}");
    let value: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut conn)
        .await
        .expect("get state");
    assert!(value.is_some(), "OAuth state should survive server restart");
}

/// Insert receipts into the queue via the production code path (the
/// receipts_queue table), simulate a server restart, and verify the
/// drainer endpoint returns them. The drainer should also DELETE the
/// rows it returned — second drain call comes back empty.
#[tokio::test]
#[ignore]
async fn receipts_queue_persists_across_restart() {
    let (database_url, redis_url) = match test_urls() {
        Some(v) => v,
        None => {
            eprintln!("skipping: TKR_TEST_DATABASE_URL / TKR_TEST_REDIS_URL unset");
            return;
        }
    };
    reset_state(&database_url, &redis_url).await;

    // Seed the queue directly. Going through the LLM-proxy endpoint
    // would require a mock upstream + signed key plumbing that the
    // existing llm_proxy.rs tests already cover end-to-end; here we
    // care about the persistence + drain claim semantics specifically.
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect PG");
    for i in 0..3 {
        let receipt = serde_json::json!({
            "ts": 1_700_000_000_u64 + i,
            "provider": "anthropic",
            "model": "claude-3-5-sonnet",
            "status": 200,
            "input_tokens": (10 + i) as u32,
            "output_tokens": (20 + i) as u32,
            "duration_ms": 100_u64,
            "sig_version": 0,
            "signature": "",
            "signer_pubkey": "",
        });
        sqlx::query("INSERT INTO receipts_queue (receipt) VALUES ($1)")
            .bind(receipt)
            .execute(&pool)
            .await
            .expect("insert receipt");
    }
    pool.close().await;

    // Sign in to satisfy any session-gated drain endpoints. (As of
    // this writing the drain endpoint is not session-gated, but
    // adding the cookie is harmless and future-proofs the test.)
    let env = server_env(&database_url, &redis_url);
    let guard = start_server(&env).await;
    let login = ureq::post(&format!("http://127.0.0.1:{}/api/auth/login", guard.port))
        .header("content-type", "application/json")
        .send(r#"{"password":"correctbattery"}"#)
        .expect("login");
    let cookie = login
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    drop(guard);

    // Restart and drain.
    let guard = start_server(&env).await;
    let resp: serde_json::Value = ureq::post(&format!(
        "http://127.0.0.1:{}/api/v1/llm/receipts/drain",
        guard.port
    ))
    .header("cookie", &cookie)
    .send("{}")
    .expect("drain")
    .into_body()
    .read_json()
    .expect("drain json");
    assert_eq!(
        resp["count"].as_u64().unwrap_or(0),
        3,
        "all three pre-restart receipts should drain post-restart"
    );

    // Second drain should be empty — the first claim DELETE'd them.
    let resp2: serde_json::Value = ureq::post(&format!(
        "http://127.0.0.1:{}/api/v1/llm/receipts/drain",
        guard.port
    ))
    .header("cookie", &cookie)
    .send("{}")
    .expect("drain 2")
    .into_body()
    .read_json()
    .expect("drain json 2");
    assert_eq!(
        resp2["count"].as_u64().unwrap_or(99),
        0,
        "second drain should return empty (claim is delete-on-return)"
    );
}
