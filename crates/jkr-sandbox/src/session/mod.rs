//! Backend-agnostic agent-session execution. `SessionBackend` is the seam
//! between the orchestrator (jkr-server) and the isolation tier; Landlock is
//! the lightweight tier (future), Docker is the heavy tier (`docker.rs`).

use std::collections::BTreeMap;
use jkr_api::AgentEvent;

pub mod docker;

/// Fully resolved instructions for one session: the image, the argv to run,
/// the env to inject (already includes ANTHROPIC_BASE_URL + x-jkr-token +
/// resolved vault secrets), the network policy, mounts, and limits.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSpec {
    pub image: String,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub network: NetMode,
    pub workspace_mount: Option<Mount>,
    pub memory_mb: u64,
    pub timeout_s: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mount {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetMode { None, GatewayOnly, Open }

/// Opaque handle to a spawned session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionHandle {
    pub session_id: String,
    pub container_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Running,
    Exited { code: i32 },
    Failed { msg: String },
}

/// An async stream of decoded agent events from a running session.
pub type EventStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<AgentEvent, SessionError>> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("event decode: {0}")]
    Decode(String),
}

/// The seam. One impl per isolation tier.
#[async_trait::async_trait]
pub trait SessionBackend: Send + Sync {
    async fn spawn(&self, spec: &SessionSpec) -> Result<SessionHandle, SessionError>;
    async fn attach_stream(&self, h: &SessionHandle) -> Result<EventStream, SessionError>;
    async fn stop(&self, h: &SessionHandle) -> Result<(), SessionError>;
    async fn status(&self, h: &SessionHandle) -> Result<SessionStatus, SessionError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_constructible_and_comparable() {
        let spec = SessionSpec {
            image: "img".into(),
            argv: vec!["jkr".into(), "agent".into(), "run".into(), "--stream".into()],
            env: BTreeMap::new(),
            network: NetMode::GatewayOnly,
            workspace_mount: None,
            memory_mb: 512,
            timeout_s: 600,
        };
        assert_eq!(spec.network, NetMode::GatewayOnly);
        assert!(spec.workspace_mount.is_none());
    }
}
