//! Session lifecycle: spawn, persist events, owner-scoped reads, reconcile on
//! restart. The vault integration for `vault:PATH` secret references is
//! stubbed — non-vault secrets pass through directly; vault-prefixed values
//! return an error at spec-build time (known limitation, v1).

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use jkr_agent::manifest::{Manifest, NetworkPolicy, SandboxBackend};
use jkr_api::AgentEvent;
use jkr_sandbox::session::{
    EventStream, Mount, NetMode, SessionBackend, SessionHandle, SessionSpec, SessionStatus,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::AgentRegistry;

pub struct SessionManager {
    pool: PgPool,
    registry: Arc<AgentRegistry>,
    backend: Arc<dyn SessionBackend>,
}

impl SessionManager {
    pub fn new(pool: PgPool, registry: Arc<AgentRegistry>, backend: Arc<dyn SessionBackend>) -> Self {
        Self { pool, registry, backend }
    }

    // ── spec construction ────────────────────────────────────────────────────

    fn build_spec(
        &self,
        m: &Manifest,
        input: &str,
        owner_token: &str,
        gateway: &str,
    ) -> Result<SessionSpec> {
        let sb = m.sandbox.as_ref().ok_or_else(|| anyhow!("manifest has no [sandbox] block"))?;
        if sb.backend != SandboxBackend::Docker {
            return Err(anyhow!("only docker backend wired in v1"));
        }

        let mut env = BTreeMap::new();
        env.insert("ANTHROPIC_BASE_URL".into(), gateway.to_string());
        env.insert("ANTHROPIC_API_KEY".into(), owner_token.to_string());
        env.insert("JKR_AGENT_INPUT".into(), input.to_string());

        for (k, v) in &m.secrets {
            if v.starts_with("vault:") {
                return Err(anyhow!("vault resolution not wired: {v}"));
            }
            env.insert(k.clone(), v.clone());
        }

        Ok(SessionSpec {
            image: sb.image.clone(),
            argv: vec![
                "jkr".into(),
                "agent".into(),
                "run".into(),
                "/manifest.toml".into(),
                "--stream".into(),
            ],
            env,
            network: map_net(sb.network),
            workspace_mount: Some(Mount {
                host_path: "/var/lib/jkr/workspaces".into(),
                container_path: "/workspace".into(),
                read_only: false,
            }),
            memory_mb: sb.memory_mb,
            timeout_s: sb.timeout_s,
        })
    }

    // ── public API ───────────────────────────────────────────────────────────

    pub async fn create(
        &self,
        owner: &str,
        agent: &str,
        version: Option<i64>,
        input: &str,
    ) -> Result<String> {
        let (ver, manifest) = self.registry.load(owner, agent, version).await?;
        let gateway = std::env::var("JKR_GATEWAY_URL")
            .unwrap_or_else(|_| "http://jkr-server:8080".into());
        let spec = self.build_spec(&manifest, input, owner, &gateway)?;

        let handle = self
            .backend
            .spawn(&spec)
            .await
            .map_err(|e| anyhow!("spawn: {e}"))?;

        let sid = Uuid::new_v4().to_string();
        let now = now_secs();
        sqlx::query(
            "INSERT INTO agent_sessions \
             (id, owner, agent_name, agent_version, state, container_id, created_at) \
             VALUES ($1, $2, $3, $4, 'running', $5, $6)",
        )
        .bind(&sid)
        .bind(owner)
        .bind(agent)
        .bind(ver)
        .bind(&handle.container_id)
        .bind(now)
        .execute(&self.pool)
        .await?;

        // Detached drain: survives regardless of the caller.
        let child = self.clone_handles();
        let sid2 = sid.clone();
        let handle2 = handle.clone();
        tokio::spawn(async move {
            if let Err(e) = child.drain(&sid2, &handle2).await {
                eprintln!("agent session {sid2} drain error: {e}");
                let _ = child.mark_failed(&sid2, &e.to_string()).await;
            }
        });

        Ok(sid)
    }

    pub async fn events_since(
        &self,
        owner: &str,
        sid: &str,
        after: i64,
    ) -> Result<Vec<AgentEvent>> {
        self.assert_owner(owner, sid).await?;
        let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
            "SELECT payload FROM agent_session_events \
             WHERE session_id = $1 AND seq >= $2 ORDER BY seq",
        )
        .bind(sid)
        .bind(after)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(v,)| serde_json::from_value(v).map_err(|e| anyhow!("decode: {e}")))
            .collect()
    }

    pub async fn status_of(&self, owner: &str, sid: &str) -> Result<String> {
        self.assert_owner(owner, sid).await?;
        let state: String =
            sqlx::query_scalar("SELECT state FROM agent_sessions WHERE id = $1")
                .bind(sid)
                .fetch_one(&self.pool)
                .await?;
        Ok(state)
    }

    /// All sessions owned by `owner`, newest first. Returns
    /// `(id, agent_name, state)` tuples.
    pub async fn list_sessions(&self, owner: &str) -> Result<Vec<(String, String, String)>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, agent_name, state FROM agent_sessions \
             WHERE owner = $1 ORDER BY created_at DESC",
        )
        .bind(owner)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Stop a running session: assert ownership, stop the container, mark
    /// state='stopped'. Owner-scoped — errors if the session isn't owned
    /// by `owner` (same "not found" shape as the other reads).
    pub async fn stop_session(&self, owner: &str, sid: &str) -> Result<()> {
        self.assert_owner(owner, sid).await?;
        let container_id: Option<String> =
            sqlx::query_scalar("SELECT container_id FROM agent_sessions WHERE id = $1")
                .bind(sid)
                .fetch_optional(&self.pool)
                .await?;
        if let Some(container_id) = container_id {
            let handle = SessionHandle {
                session_id: sid.to_string(),
                container_id,
            };
            let _ = self.backend.stop(&handle).await;
        }
        let now = now_secs();
        sqlx::query("UPDATE agent_sessions SET state = 'stopped', ended_at = $1 WHERE id = $2")
            .bind(now)
            .bind(sid)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn reconcile(&self) -> Result<()> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, container_id FROM agent_sessions WHERE state = 'running'")
                .fetch_all(&self.pool)
                .await?;
        for (sid, container_id) in rows {
            let handle = SessionHandle { session_id: sid.clone(), container_id };
            let child = self.clone_handles();
            tokio::spawn(async move {
                if let Err(e) = child.drain(&sid, &handle).await {
                    eprintln!("agent session {sid} drain error: {e}");
                    let _ = child.mark_failed(&sid, &e.to_string()).await;
                }
            });
        }
        Ok(())
    }

    // expose for deterministic testing
    #[doc(hidden)]
    pub async fn drain_for_test(&self, sid: &str, handle: &SessionHandle) -> Result<()> {
        self.drain(sid, handle).await
    }

    // ── internals ────────────────────────────────────────────────────────────

    async fn drain(&self, sid: &str, handle: &SessionHandle) -> Result<()> {
        let stream: EventStream = self
            .backend
            .attach_stream(handle)
            .await
            .map_err(|e| anyhow!("attach: {e}"))?;

        use futures_util::StreamExt;
        let mut pinned = std::pin::pin!(stream);

        while let Some(item) = pinned.next().await {
            match item {
                Ok(ev) => {
                    self.persist_event(sid, &ev).await?;
                }
                Err(e) => {
                    self.mark_failed(sid, &e.to_string()).await;
                    let _ = self.backend.stop(handle).await;
                    return Ok(());
                }
            }
        }

        self.finalize(sid, handle).await;
        Ok(())
    }

    async fn persist_event(&self, sid: &str, ev: &AgentEvent) -> Result<()> {
        let payload = serde_json::to_value(ev)?;
        sqlx::query(
            "INSERT INTO agent_session_events (session_id, seq, payload, created_at) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(sid)
        .bind(ev.seq as i64)
        .bind(payload)
        .bind(now_secs())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn finalize(&self, sid: &str, handle: &SessionHandle) {
        let status = self
            .backend
            .status(handle)
            .await
            .unwrap_or(SessionStatus::Failed { msg: "status unavailable".into() });

        let (state, exit_code): (&str, Option<i32>) = match status {
            SessionStatus::Exited { code } => ("exited", Some(code)),
            SessionStatus::Running => ("exited", None),
            SessionStatus::Failed { .. } => ("failed", None),
        };

        let now = now_secs();
        let _ = sqlx::query(
            "UPDATE agent_sessions SET state = $1, exit_code = $2, ended_at = $3 WHERE id = $4",
        )
        .bind(state)
        .bind(exit_code)
        .bind(now)
        .bind(sid)
        .execute(&self.pool)
        .await;

        let _ = self.backend.stop(handle).await;
    }

    async fn mark_failed(&self, sid: &str, _msg: &str) {
        let now = now_secs();
        let _ = sqlx::query(
            "UPDATE agent_sessions SET state = 'failed', ended_at = $1 WHERE id = $2",
        )
        .bind(now)
        .bind(sid)
        .execute(&self.pool)
        .await;
    }

    async fn assert_owner(&self, owner: &str, sid: &str) -> Result<()> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT owner FROM agent_sessions WHERE id = $1")
                .bind(sid)
                .fetch_optional(&self.pool)
                .await?;
        match row {
            Some((o,)) if o == owner => Ok(()),
            _ => Err(anyhow!("session not found")),
        }
    }

    fn clone_handles(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            registry: Arc::clone(&self.registry),
            backend: Arc::clone(&self.backend),
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn map_net(p: NetworkPolicy) -> NetMode {
    match p {
        NetworkPolicy::None => NetMode::None,
        NetworkPolicy::GatewayOnly => NetMode::GatewayOnly,
        NetworkPolicy::Open => NetMode::Open,
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use jkr_api::AgentEventKind;
    use jkr_sandbox::session::SessionError;
    use sqlx::postgres::PgPoolOptions;

    struct MockBackend;

    #[async_trait::async_trait]
    impl SessionBackend for MockBackend {
        async fn spawn(&self, _spec: &SessionSpec) -> Result<SessionHandle, SessionError> {
            Ok(SessionHandle {
                session_id: "mock-session".into(),
                container_id: "mock-container-id".into(),
            })
        }

        async fn attach_stream(
            &self,
            _h: &SessionHandle,
        ) -> Result<EventStream, SessionError> {
            let events: Vec<Result<AgentEvent, SessionError>> = vec![
                Ok(AgentEvent { seq: 0, kind: AgentEventKind::Step { step: 1, max: 3 } }),
                Ok(AgentEvent {
                    seq: 1,
                    kind: AgentEventKind::ToolOutput {
                        tool: "process".into(),
                        bytes: 42,
                        text: "hello".into(),
                    },
                }),
                Ok(AgentEvent { seq: 2, kind: AgentEventKind::Done { exit: 0, steps: 1 } }),
            ];
            use futures_util::StreamExt;
            Ok(stream::iter(events).boxed())
        }

        async fn stop(&self, _h: &SessionHandle) -> Result<(), SessionError> {
            Ok(())
        }

        async fn status(&self, _h: &SessionHandle) -> Result<SessionStatus, SessionError> {
            Ok(SessionStatus::Exited { code: 0 })
        }
    }

    #[tokio::test]
    #[ignore]
    async fn session_manager_full_lifecycle() {
        let db_url = match std::env::var("JKR_TEST_DATABASE_URL").ok() {
            Some(u) => u,
            None => {
                eprintln!("skipping: JKR_TEST_DATABASE_URL unset");
                return;
            }
        };

        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect(&db_url)
            .await
            .expect("connect");

        sqlx::migrate!("./migrations").run(&pool).await.expect("migrate");

        let owner = format!("test-{}", Uuid::new_v4());
        let registry = Arc::new(AgentRegistry::new(pool.clone()));

        let manifest_toml = r#"
name = "my-agent"
task = "do stuff"

[model]
provider = "anthropic"
name = "claude-3-5-haiku-20241022"

[sandbox]
backend = "docker"
image = "jkr-agent:latest"
network = "gateway_only"
memory_mb = 512
timeout_s = 600
"#;

        registry.push(&owner, manifest_toml).await.unwrap();

        let mgr = SessionManager::new(pool.clone(), registry, Arc::new(MockBackend));

        let sid = mgr.create(&owner, "my-agent", None, "test input").await.unwrap();

        // get the handle back out so we can drain deterministically
        let handle = SessionHandle {
            session_id: sid.clone(),
            container_id: "mock-container-id".into(),
        };
        // drain synchronously in test (create already spawned a detached task;
        // ON CONFLICT DO NOTHING keeps persist_event idempotent)
        mgr.drain_for_test(&sid, &handle).await.unwrap();

        // 3 events with seq 0,1,2
        let evs = mgr.events_since(&owner, &sid, 0).await.unwrap();
        assert_eq!(evs.len(), 3);
        assert_eq!(evs[0].seq, 0);
        assert_eq!(evs[1].seq, 1);
        assert_eq!(evs[2].seq, 2);
        assert!(matches!(evs[2].kind, AgentEventKind::Done { .. }));

        // last event is Done
        assert!(matches!(evs.last().unwrap().kind, AgentEventKind::Done { .. }));

        // state is exited
        let state = mgr.status_of(&owner, &sid).await.unwrap();
        assert_eq!(state, "exited");

        // cross-owner rejected
        assert!(mgr.events_since("other-owner", &sid, 0).await.is_err());

        pool.close().await;
    }
}
