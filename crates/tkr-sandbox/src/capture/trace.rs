use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind { Full, DenialsOnly, None }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOp { Read, Write, Stat, Delete, Rename }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetFamily { V4, V6, Unix }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEvent {
    pub path: String,
    pub op: FileOp,
    pub allowed: bool,
    pub errno: Option<i32>,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetEvent {
    pub addr: String,
    pub family: NetFamily,
    pub allowed: bool,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecEvent {
    pub argv0: String,
    pub argv_preview: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSummary {
    pub files_total: u32,
    pub net_total: u32,
    pub exec_total: u32,
    pub denied_total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictLevel { Clean, Notable, Suspicious }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flag {
    pub kind: String,
    pub severity: VerdictLevel,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub level: VerdictLevel,
    pub flags: Vec<Flag>,
}

/// Per-category event cap. Past this, append stops and `truncated` is set.
pub const TRACE_EVENT_CAP: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxTrace {
    pub files: Vec<FileEvent>,
    pub net: Vec<NetEvent>,
    pub execs: Vec<ExecEvent>,
    pub summary: TraceSummary,
    pub verdict: Verdict,
    pub capture_kind: CaptureKind,
    pub truncated: bool,
}

impl SandboxTrace {
    /// An empty trace for platforms/backends that captured nothing.
    pub fn none() -> Self {
        SandboxTrace {
            files: Vec::new(),
            net: Vec::new(),
            execs: Vec::new(),
            summary: TraceSummary::default(),
            verdict: Verdict { level: VerdictLevel::Clean, flags: Vec::new() },
            capture_kind: CaptureKind::None,
            truncated: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_preserves_trace() {
        let t = SandboxTrace {
            files: vec![FileEvent {
                path: "/etc/passwd".into(), op: FileOp::Read,
                allowed: true, errno: None, count: 2,
            }],
            net: vec![NetEvent {
                addr: "93.184.216.34:443".into(), family: NetFamily::V4,
                allowed: true, count: 1,
            }],
            execs: vec![ExecEvent { argv0: "node".into(), argv_preview: "node index.js".into() }],
            summary: TraceSummary { files_total: 1, net_total: 1, exec_total: 1, denied_total: 0 },
            verdict: Verdict { level: VerdictLevel::Notable, flags: vec![Flag {
                kind: "egress".into(), severity: VerdictLevel::Notable,
                detail: "connect to 93.184.216.34:443".into(),
            }] },
            capture_kind: CaptureKind::Full,
            truncated: false,
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: SandboxTrace = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn none_is_clean_and_empty() {
        let t = SandboxTrace::none();
        assert_eq!(t.capture_kind, CaptureKind::None);
        assert_eq!(t.verdict.level, VerdictLevel::Clean);
        assert!(t.files.is_empty());
    }
}
