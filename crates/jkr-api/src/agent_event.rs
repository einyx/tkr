//! The NDJSON event contract emitted by a streaming agent run and consumed
//! by the session orchestrator. One JSON object per line.

use serde::{Deserialize, Serialize};

/// A single event in an agent run. `seq` is assigned by the emitter,
/// monotonic from 0, and used for ordering, persistence, and SSE resume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub kind: AgentEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AgentEventKind {
    Step { step: u32, max: u32 },
    Token { text: String },
    ToolStart { tool: String, args: serde_json::Value },
    ToolOutput { tool: String, bytes: usize, text: String },
    Done { exit: i32, steps: u32 },
    Error { kind: String, msg: String },
}

impl AgentEvent {
    /// Serialize as a single NDJSON line (no trailing newline).
    pub fn to_ndjson(&self) -> String {
        serde_json::to_string(self).expect("AgentEvent serializes")
    }

    /// Parse one NDJSON line.
    pub fn from_ndjson(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_ndjson() {
        let ev = AgentEvent {
            seq: 3,
            kind: AgentEventKind::ToolOutput {
                tool: "process".into(),
                bytes: 214,
                text: "hello".into(),
            },
        };
        let line = ev.to_ndjson();
        assert!(line.contains("\"seq\":3"));
        assert!(line.contains("\"t\":\"tool_output\""));
        assert_eq!(AgentEvent::from_ndjson(&line).unwrap(), ev);
    }

    #[test]
    fn done_event_shape() {
        let ev = AgentEvent { seq: 7, kind: AgentEventKind::Done { exit: 0, steps: 5 } };
        let v: serde_json::Value = serde_json::from_str(&ev.to_ndjson()).unwrap();
        assert_eq!(v["t"], "done");
        assert_eq!(v["exit"], 0);
        assert_eq!(v["steps"], 5);
        assert_eq!(v["seq"], 7);
    }
}
