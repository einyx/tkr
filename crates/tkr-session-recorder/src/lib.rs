//! tkr-session-recorder — captures AI agent tool-call events into the vault.
//!
//! Two write paths share the same on-disk schema:
//!
//! 1. The in-process [`SessionRecorderPluginV2`] subscribes to `on_command_*`
//!    lifecycle hooks and records Bash commands run through the tkr proxy.
//! 2. The out-of-process `tkr hook claude` / `tkr hook universal` commands call
//!    [`storage::append_event`] directly to record non-Bash tool calls
//!    (Read, Edit, WebFetch, etc.).
//!
//! Storage layout:
//! ```text
//! ~/.tkr/vault/<encrypted>/sessions/<session_id>/
//!   meta.json       — SessionMeta
//!   events.jsonl    — append-only Event stream
//! ```

pub mod scrub;
pub mod storage;

pub use storage::{
    append_event, estimate_tokens, list_sessions, make_event, new_session_id, read_events,
    read_meta, write_meta, Event, SessionMeta,
};

use std::sync::{Arc, Mutex};
use std::time::Instant;
use tkr_api::{
    capability,
    host::Host,
    manifest::{Manifest, SensitivityClass, StorageKind, StorageRequest},
    plugin::{CommandCtx, FilterDecision, Plugin},
    Result as ApiResult,
};

/// Environment variable consulted for the active session ID. Set by the hook
/// command when running under Claude Code (which propagates `session_id` in
/// its hook input). When unset, the plugin generates a per-process ID on first
/// command and reuses it for the lifetime of the process.
pub const SESSION_ID_ENV: &str = "TKR_SESSION_ID";

/// Environment variable for the agent label (e.g. `claude-code`, `cursor`,
/// `manual`). Defaults to `manual` when unset.
pub const AGENT_ENV: &str = "TKR_AGENT";

pub struct SessionRecorderPluginV2 {
    host: Mutex<Option<Arc<dyn Host>>>,
    /// Resolved once on first command_begin and cached.
    session_id: Mutex<Option<String>>,
    /// Per-command state between begin and end.
    in_flight: Mutex<Option<InFlight>>,
    /// Next sequence number for the active session.
    next_seq: Mutex<u64>,
}

struct InFlight {
    started_at: Instant,
    command: String,
    args: String,
    chars_in: u64,
    filter_savings_chars: u64,
    output_preview: String,
    /// True when the command name is on the deny list (cat / env / printenv
    /// / etc.) — `output_preview` is suppressed for the lifetime of the
    /// command and replaced with a single explanatory marker on completion.
    suppress_preview: bool,
}

impl SessionRecorderPluginV2 {
    pub fn new() -> Self {
        Self {
            host: Mutex::new(None),
            session_id: Mutex::new(None),
            in_flight: Mutex::new(None),
            next_seq: Mutex::new(0),
        }
    }

    /// Resolve the active session ID — env var first, then a freshly-generated
    /// one cached for the rest of the process. On first resolution this also
    /// writes `meta.json` so the session is discoverable by `tkr replay`.
    fn resolve_session(&self, host: &dyn Host) -> String {
        let mut guard = self.session_id.lock().unwrap();
        if let Some(id) = guard.as_ref() {
            return id.clone();
        }
        let id = std::env::var(SESSION_ID_ENV).unwrap_or_else(|_| storage::new_session_id());

        // Best-effort meta write. If the meta already exists (e.g. another
        // tkr invocation in the same session set it), don't overwrite the
        // started_at — preserve the original.
        let existing = storage::read_meta(host, &id).ok().flatten();
        let meta = SessionMeta {
            session_id: id.clone(),
            started_at: existing
                .as_ref()
                .map(|m| m.started_at.clone())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
            ended_at: existing.and_then(|m| m.ended_at),
            agent: std::env::var(AGENT_ENV).unwrap_or_else(|_| "manual".into()),
            project_root: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned()),
            tkr_version: env!("CARGO_PKG_VERSION").into(),
        };
        let _ = storage::write_meta(host, &meta);

        *guard = Some(id.clone());
        id
    }

    fn next_seq(&self) -> u64 {
        let mut g = self.next_seq.lock().unwrap();
        let s = *g;
        *g = s.saturating_add(1);
        s
    }
}

impl Default for SessionRecorderPluginV2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for SessionRecorderPluginV2 {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "tkr-session-recorder".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities_required: vec![
                capability::VAULT_READ_SECRET.into(),
                capability::VAULT_WRITE_SECRET.into(),
                capability::STDOUT_FILTER.into(),
            ],
            storage_requests: vec![StorageRequest {
                kind: StorageKind::Fs,
                class: SensitivityClass::Secret,
            }],
            ..Default::default()
        }
    }

    fn on_load(&mut self, host: Arc<dyn Host>) -> ApiResult<()> {
        *self.host.lock().unwrap() = Some(host);
        Ok(())
    }

    fn on_command_begin(&mut self, ctx: &CommandCtx) -> ApiResult<()> {
        let suppress = scrub::is_deny_listed(&ctx.command);
        *self.in_flight.lock().unwrap() = Some(InFlight {
            started_at: Instant::now(),
            command: ctx.command.clone(),
            args: ctx.args.clone(),
            chars_in: 0,
            filter_savings_chars: 0,
            output_preview: String::new(),
            suppress_preview: suppress,
        });
        Ok(())
    }

    fn on_line(&mut self, line: &str, _ctx: &CommandCtx) -> ApiResult<FilterDecision> {
        if let Some(inflight) = self.in_flight.lock().unwrap().as_mut() {
            inflight.chars_in += line.len() as u64;
            if inflight.suppress_preview {
                return Ok(FilterDecision::Pass);
            }
            // Keep the first ~2 KB as a preview snapshot — matches design §5.2.
            // Each line is run through scrub::scrub_line so common API-key /
            // bearer-token / PEM shapes are replaced with `<redacted: …>`
            // before they reach the encrypted-but-recoverable preview.
            let scrubbed = scrub::scrub_line(line);
            if inflight.output_preview.len() < 2048 {
                let remaining = 2048 - inflight.output_preview.len();
                let take = scrubbed.len().min(remaining);
                inflight.output_preview.push_str(&scrubbed[..take]);
                if take < scrubbed.len() {
                    inflight.output_preview.push('\n');
                }
            }
        }
        Ok(FilterDecision::Pass)
    }

    fn on_command_end(&mut self, _ctx: &CommandCtx) -> ApiResult<String> {
        let host_guard = self.host.lock().unwrap();
        let Some(host) = host_guard.as_ref().cloned() else {
            return Ok(String::new());
        };
        drop(host_guard);

        let inflight = match self.in_flight.lock().unwrap().take() {
            Some(i) => i,
            None => return Ok(String::new()),
        };

        let session_id = self.resolve_session(host.as_ref());
        let seq = self.next_seq();

        let input_str = if inflight.args.is_empty() {
            inflight.command.clone()
        } else {
            format!("{} {}", inflight.command, inflight.args)
        };

        let preview = if inflight.suppress_preview {
            "<preview suppressed: command in deny-list>".to_string()
        } else {
            inflight.output_preview
        };
        let event = storage::make_event(
            session_id,
            seq,
            "Bash",
            serde_json::Value::String(input_str),
            preview,
            0, // tokens_in: command-line input is tiny; left at 0 for v1
            (inflight.chars_in / 4) as u32,
            (inflight.filter_savings_chars / 4) as u32,
            inflight.started_at.elapsed().as_millis() as u32,
            None,
        );

        // Best-effort write — never fail a command because recording failed.
        let _ = storage::append_event(host.as_ref(), &event);

        Ok(String::new())
    }
}

#[cfg(test)]
#[cfg(feature = "test-host")]
mod tests {
    use super::*;
    use tkr_api::test_host::TestHost;

    fn ctx(command: &str, args: &str) -> CommandCtx {
        CommandCtx {
            command: command.into(),
            args: args.into(),
            line_index: 0,
        }
    }

    #[test]
    fn writes_event_on_command_end() {
        // Use a fixed session ID so the test is deterministic.
        std::env::set_var(SESSION_ID_ENV, "test-session-001");
        std::env::set_var(AGENT_ENV, "test");

        let host: Arc<dyn Host + 'static> = Arc::new(TestHost::new("tkr-session-recorder"));
        let mut p = SessionRecorderPluginV2::new();
        p.on_load(host.clone()).unwrap();

        p.on_command_begin(&ctx("git", "status")).unwrap();
        p.on_line("M  Cargo.toml", &ctx("git", "status")).unwrap();
        p.on_line("?? new.rs", &ctx("git", "status")).unwrap();
        p.on_command_end(&ctx("git", "status")).unwrap();

        let events = storage::read_events(host.as_ref(), "test-session-001").unwrap();
        assert_eq!(events.len(), 1, "one event expected");
        assert_eq!(events[0].tool, "Bash");
        assert_eq!(
            events[0].input.as_str(),
            Some("git status"),
            "input should be the joined command"
        );
        assert!(events[0].output_preview.contains("Cargo.toml"));

        std::env::remove_var(SESSION_ID_ENV);
        std::env::remove_var(AGENT_ENV);
    }

    #[test]
    fn assigns_monotonic_seq() {
        std::env::set_var(SESSION_ID_ENV, "test-session-002");
        let host: Arc<dyn Host + 'static> = Arc::new(TestHost::new("tkr-session-recorder"));
        let mut p = SessionRecorderPluginV2::new();
        p.on_load(host.clone()).unwrap();

        for i in 0..3 {
            p.on_command_begin(&ctx("ls", "")).unwrap();
            p.on_line(&format!("file-{i}"), &ctx("ls", "")).unwrap();
            p.on_command_end(&ctx("ls", "")).unwrap();
        }

        let events = storage::read_events(host.as_ref(), "test-session-002").unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[1].seq, 1);
        assert_eq!(events[2].seq, 2);

        std::env::remove_var(SESSION_ID_ENV);
    }

    #[test]
    fn meta_written_on_first_event() {
        std::env::set_var(SESSION_ID_ENV, "test-session-003");
        std::env::set_var(AGENT_ENV, "claude-code");

        let host: Arc<dyn Host + 'static> = Arc::new(TestHost::new("tkr-session-recorder"));
        let mut p = SessionRecorderPluginV2::new();
        p.on_load(host.clone()).unwrap();

        p.on_command_begin(&ctx("echo", "hi")).unwrap();
        p.on_line("hi", &ctx("echo", "hi")).unwrap();
        p.on_command_end(&ctx("echo", "hi")).unwrap();

        let meta = storage::read_meta(host.as_ref(), "test-session-003")
            .unwrap()
            .expect("meta should exist");
        assert_eq!(meta.session_id, "test-session-003");
        assert_eq!(meta.agent, "claude-code");

        std::env::remove_var(SESSION_ID_ENV);
        std::env::remove_var(AGENT_ENV);
    }
}

#[cfg(test)]
mod storage_unit_tests {
    use super::storage::*;

    #[test]
    fn new_session_id_is_unique_and_sortable() {
        let a = new_session_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = new_session_id();
        assert_ne!(a, b);
        // Hex-encoded ms timestamp prefix means lexical sort = chronological sort.
        assert!(a < b, "expected {a} < {b}");
    }

    #[test]
    fn estimate_tokens_is_bytes_over_four() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }
}
