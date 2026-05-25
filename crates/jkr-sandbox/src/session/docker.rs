//! Docker implementation of SessionBackend using bollard 0.21.

use std::env;

use bollard::Docker;
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    StopContainerOptionsBuilder,
};
use futures_util::StreamExt;
use jkr_api::AgentEvent;

use super::{EventStream, Mount, NetMode, SessionBackend, SessionError, SessionHandle,
            SessionSpec, SessionStatus};

pub struct DockerBackend {
    docker: Docker,
}

impl DockerBackend {
    pub fn connect() -> Result<Self, SessionError> {
        let docker = if env::var("DOCKER_HOST").is_ok() {
            Docker::connect_with_defaults()
        } else {
            Docker::connect_with_local_defaults()
        }
        .map_err(|e| SessionError::Backend(e.to_string()))?;
        Ok(Self { docker })
    }
}

fn network_name(mode: NetMode) -> &'static str {
    match mode {
        NetMode::None => "none",
        NetMode::GatewayOnly => "jkr-gateway",
        NetMode::Open => "bridge",
    }
}

/// Buffers incomplete NDJSON lines across chunks and emits complete parsed events.
pub(crate) struct LineDecoder {
    pending: String,
}

impl LineDecoder {
    pub fn new() -> Self {
        Self { pending: String::new() }
    }

    pub fn push(&mut self, chunk: &str) -> Vec<Result<AgentEvent, SessionError>> {
        self.pending.push_str(chunk);
        let mut results = Vec::new();
        loop {
            match self.pending.find('\n') {
                None => break,
                Some(pos) => {
                    let line: String = self.pending.drain(..=pos).collect();
                    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                    if trimmed.is_empty() {
                        continue;
                    }
                    results.push(
                        AgentEvent::from_ndjson(trimmed)
                            .map_err(|e| SessionError::Decode(e.to_string())),
                    );
                }
            }
        }
        results
    }
}

fn mount_bind(m: &Mount) -> String {
    if m.read_only {
        format!("{}:{}:ro", m.host_path, m.container_path)
    } else {
        format!("{}:{}", m.host_path, m.container_path)
    }
}

#[async_trait::async_trait]
impl SessionBackend for DockerBackend {
    async fn spawn(&self, spec: &SessionSpec) -> Result<SessionHandle, SessionError> {
        let env_vec: Vec<String> = spec
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        let binds: Option<Vec<String>> = spec
            .workspace_mount
            .as_ref()
            .map(|m| vec![mount_bind(m)]);

        let memory_bytes = (spec.memory_mb * 1024 * 1024) as i64;

        let host_config = HostConfig {
            network_mode: Some(network_name(spec.network).to_string()),
            memory: Some(memory_bytes),
            binds,
            auto_remove: Some(false),
            ..Default::default()
        };

        let body = ContainerCreateBody {
            image: Some(spec.image.clone()),
            cmd: Some(spec.argv.clone()),
            env: Some(env_vec),
            host_config: Some(host_config),
            ..Default::default()
        };

        let options = CreateContainerOptionsBuilder::default().build();

        let response = self
            .docker
            .create_container(Some(options), body)
            .await
            .map_err(|e| SessionError::Backend(e.to_string()))?;

        let container_id = response.id;

        self.docker
            .start_container(&container_id, None)
            .await
            .map_err(|e| SessionError::Backend(e.to_string()))?;

        Ok(SessionHandle {
            session_id: String::new(),
            container_id,
        })
    }

    async fn attach_stream(&self, h: &SessionHandle) -> Result<EventStream, SessionError> {
        let options = LogsOptionsBuilder::default()
            .follow(true)
            .stdout(true)
            .stderr(true)
            .build();

        let log_stream = self.docker.logs(&h.container_id, Some(options));

        let state = (log_stream, LineDecoder::new(), std::collections::VecDeque::<Result<AgentEvent, SessionError>>::new());
        let stream = futures_util::stream::unfold(state, |(mut logs, mut decoder, mut queue)| async move {
            loop {
                if let Some(ev) = queue.pop_front() {
                    return Some((ev, (logs, decoder, queue)));
                }
                match logs.next().await {
                    Some(Ok(frame)) => {
                        let bytes = frame.into_bytes();
                        let text = String::from_utf8_lossy(&bytes);
                        for ev in decoder.push(&text) {
                            queue.push_back(ev);
                        }
                    }
                    Some(Err(e)) => {
                        return Some((Err(SessionError::Backend(e.to_string())), (logs, decoder, queue)));
                    }
                    None => return None,
                }
            }
        });

        Ok(Box::pin(stream))
    }

    async fn stop(&self, h: &SessionHandle) -> Result<(), SessionError> {
        let stop_opts = StopContainerOptionsBuilder::default().t(5).build();
        let _ = self.docker.stop_container(&h.container_id, Some(stop_opts)).await;

        let rm_opts = RemoveContainerOptionsBuilder::default().force(true).build();
        let _ = self.docker.remove_container(&h.container_id, Some(rm_opts)).await;

        Ok(())
    }

    async fn status(&self, h: &SessionHandle) -> Result<SessionStatus, SessionError> {
        use bollard::models::ContainerStateStatusEnum;

        let info = self
            .docker
            .inspect_container(&h.container_id, None)
            .await
            .map_err(|e| SessionError::Backend(e.to_string()))?;

        let state = info.state.ok_or_else(|| SessionError::Backend("no state".into()))?;

        match state.status {
            Some(ContainerStateStatusEnum::RUNNING) => Ok(SessionStatus::Running),
            Some(ContainerStateStatusEnum::EXITED) => {
                let code = state.exit_code.unwrap_or(0) as i32;
                Ok(SessionStatus::Exited { code })
            }
            Some(other) => Ok(SessionStatus::Failed { msg: other.to_string() }),
            None => Ok(SessionStatus::Failed { msg: "unknown".into() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jkr_api::AgentEventKind;

    #[test]
    fn decoder_splits_lines_and_buffers_partials() {
        let mut d = LineDecoder::new();
        let mut evs = d.push("{\"seq\":0,\"t\":\"step\",\"step\":1,\"max\":5}\n{\"seq\":1,");
        assert_eq!(evs.len(), 1);
        assert!(matches!(
            evs.remove(0).unwrap().kind,
            AgentEventKind::Step { .. }
        ));
        let mut evs2 = d.push("\"t\":\"done\",\"exit\":0,\"steps\":1}\n");
        assert_eq!(evs2.len(), 1);
        assert!(matches!(
            evs2.remove(0).unwrap().kind,
            AgentEventKind::Done { .. }
        ));
    }

    #[test]
    fn mount_bind_formats_ro_and_rw() {
        let ro = Mount { host_path: "/h".into(), container_path: "/c".into(), read_only: true };
        let rw = Mount { host_path: "/h".into(), container_path: "/c".into(), read_only: false };
        assert_eq!(mount_bind(&ro), "/h:/c:ro");
        assert_eq!(mount_bind(&rw), "/h:/c");
    }

    #[tokio::test]
    #[ignore]
    async fn spawns_echoes_and_reports_exit() {
        let backend = DockerBackend::connect().unwrap();
        let spec = SessionSpec {
            image: "alpine:latest".into(),
            argv: vec![
                "sh".into(),
                "-c".into(),
                "echo '{\"seq\":0,\"t\":\"done\",\"exit\":0,\"steps\":0}'".into(),
            ],
            env: Default::default(),
            network: NetMode::None,
            workspace_mount: None,
            memory_mb: 64,
            timeout_s: 30,
        };
        let h = backend.spawn(&spec).await.unwrap();
        let mut stream = backend.attach_stream(&h).await.unwrap();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.seq, 0);
        backend.stop(&h).await.unwrap();
    }
}
