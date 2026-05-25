pub mod trace;
pub mod verdict;

#[cfg(target_os = "linux")]
pub mod linux_ptrace;

use crate::error::SandboxError;
use crate::exec::SandboxOutput;
use crate::policy::SandboxPolicy;
use trace::SandboxTrace;

/// A platform backend that runs a command under sandbox policy and returns
/// both the captured output and a behavioral trace.
pub trait CaptureBackend {
    fn run(
        &self,
        command: &str,
        args: &[&str],
        policy: &SandboxPolicy,
    ) -> Result<(SandboxOutput, SandboxTrace), SandboxError>;
}

/// Run under the platform's capture backend. On Linux this is the ptrace
/// backend (full trace); elsewhere it falls back to the output-only path with
/// an empty `CaptureKind::None` trace.
pub fn run_with_capture(
    command: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<(SandboxOutput, SandboxTrace), SandboxError> {
    #[cfg(target_os = "linux")]
    {
        linux_ptrace::LinuxPtraceBackend.run(command, args, policy)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let out = crate::exec::run_sandboxed_output_only(command, args, policy)?;
        Ok((out, SandboxTrace::none()))
    }
}

use std::collections::HashMap;
use trace::{
    ExecEvent, FileEvent, FileOp, NetEvent, TraceSummary, Verdict, VerdictLevel, TRACE_EVENT_CAP,
};

#[allow(dead_code)]
pub(crate) enum PendingKind {
    File { path: String, op: FileOp },
    Net { addr: String, family: trace::NetFamily },
    Exec { argv0: String, argv_preview: String },
    Ignore,
}
#[allow(dead_code)]
pub(crate) struct Pending {
    pub kind: PendingKind,
}

#[allow(dead_code)]
pub(crate) struct EventCollector {
    files: Vec<FileEvent>,
    net: Vec<NetEvent>,
    execs: Vec<ExecEvent>,
    truncated: bool,
    pending: HashMap<i32, Pending>,
    writable_roots: Vec<String>,
}

impl EventCollector {
    pub(crate) fn new(writable_roots: Vec<String>) -> Self {
        EventCollector {
            files: Vec::new(),
            net: Vec::new(),
            execs: Vec::new(),
            truncated: false,
            pending: HashMap::new(),
            writable_roots,
        }
    }

    // Real syscall decoding lands in Task 5. Empty for now so the loop compiles.
    #[cfg(target_os = "linux")]
    pub(crate) fn on_entry(&mut self, _pid: nix::unistd::Pid) {}
    #[cfg(target_os = "linux")]
    pub(crate) fn on_exit(&mut self, _pid: nix::unistd::Pid) {}

    pub(crate) fn finish(self) -> SandboxTrace {
        let denied_total = self
            .files
            .iter()
            .filter(|f| !f.allowed)
            .map(|f| f.count)
            .sum::<u32>()
            + self
                .net
                .iter()
                .filter(|n| !n.allowed)
                .map(|n| n.count)
                .sum::<u32>();
        let _ = TRACE_EVENT_CAP;
        SandboxTrace {
            summary: TraceSummary {
                files_total: self.files.iter().map(|f| f.count).sum(),
                net_total: self.net.iter().map(|n| n.count).sum(),
                exec_total: self.execs.len() as u32,
                denied_total,
            },
            files: self.files,
            net: self.net,
            execs: self.execs,
            verdict: Verdict {
                level: VerdictLevel::Clean,
                flags: vec![],
            },
            capture_kind: trace::CaptureKind::Full,
            truncated: self.truncated,
        }
    }
}
