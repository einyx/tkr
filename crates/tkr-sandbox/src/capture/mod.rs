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

// Push/dedup helpers and the test-only constructor are NOT linux-gated so the
// dedup unit test compiles and runs on every platform.
impl EventCollector {
    #[cfg(test)]
    pub(crate) fn new_for_test(writable_roots: Vec<String>) -> Self {
        EventCollector::new(writable_roots)
    }

    pub(crate) fn push_file(&mut self, path: &str, op: FileOp, allowed: bool, errno: Option<i32>) {
        if let Some(e) = self
            .files
            .iter_mut()
            .find(|e| e.path == path && e.op == op && e.allowed == allowed)
        {
            e.count += 1;
            return;
        }
        if self.files.len() >= TRACE_EVENT_CAP {
            self.truncated = true;
            return;
        }
        self.files.push(FileEvent {
            path: path.into(),
            op,
            allowed,
            errno,
            count: 1,
        });
    }

    pub(crate) fn push_net(&mut self, addr: &str, family: trace::NetFamily, allowed: bool) {
        if let Some(e) = self
            .net
            .iter_mut()
            .find(|e| e.addr == addr && e.allowed == allowed)
        {
            e.count += 1;
            return;
        }
        if self.net.len() >= TRACE_EVENT_CAP {
            self.truncated = true;
            return;
        }
        self.net.push(NetEvent {
            addr: addr.into(),
            family,
            allowed,
            count: 1,
        });
    }

    pub(crate) fn push_exec(&mut self, argv0: String, argv_preview: String) {
        if self.execs.len() >= TRACE_EVENT_CAP {
            self.truncated = true;
            return;
        }
        self.execs.push(ExecEvent { argv0, argv_preview });
    }
}

#[cfg(target_os = "linux")]
impl EventCollector {
    pub(crate) fn on_entry(&mut self, pid: nix::unistd::Pid) {
        use crate::capture::linux_ptrace as lp;
        use syscalls::Sysno;
        let Some(regs) = lp::getregs(pid) else {
            return;
        };
        let nr = lp::syscall_nr(&regs) as usize;
        // `Sysno::from(i32)` panics on unknown syscall numbers, and the tracer
        // sees every syscall — so use the fallible `Sysno::new`.
        let Some(sysno) = Sysno::new(nr) else {
            self.pending
                .insert(pid.as_raw(), Pending { kind: PendingKind::Ignore });
            return;
        };
        let kind = match sysno {
            Sysno::openat => {
                let path = lp::read_cstr(pid, lp::arg(&regs, 1));
                let flags = lp::arg(&regs, 2) as i32;
                let op = if flags & libc::O_WRONLY != 0 || flags & libc::O_RDWR != 0 {
                    FileOp::Write
                } else {
                    FileOp::Read
                };
                PendingKind::File { path, op }
            }
            Sysno::open => {
                let path = lp::read_cstr(pid, lp::arg(&regs, 0));
                let flags = lp::arg(&regs, 1) as i32;
                let op = if flags & libc::O_WRONLY != 0 || flags & libc::O_RDWR != 0 {
                    FileOp::Write
                } else {
                    FileOp::Read
                };
                PendingKind::File { path, op }
            }
            Sysno::newfstatat | Sysno::statx => PendingKind::File {
                path: lp::read_cstr(pid, lp::arg(&regs, 1)),
                op: FileOp::Stat,
            },
            Sysno::unlinkat => PendingKind::File {
                path: lp::read_cstr(pid, lp::arg(&regs, 1)),
                op: FileOp::Delete,
            },
            Sysno::connect => match lp::read_sockaddr(pid, lp::arg(&regs, 1)) {
                Some((addr, family)) => PendingKind::Net { addr, family },
                None => PendingKind::Ignore,
            },
            Sysno::execve => {
                let argv0 = lp::read_cstr(pid, lp::arg(&regs, 0));
                PendingKind::Exec {
                    argv0: argv0.clone(),
                    argv_preview: argv0,
                }
            }
            _ => PendingKind::Ignore,
        };
        self.pending.insert(pid.as_raw(), Pending { kind });
    }

    pub(crate) fn on_exit(&mut self, pid: nix::unistd::Pid) {
        use crate::capture::linux_ptrace as lp;
        let Some(p) = self.pending.remove(&pid.as_raw()) else {
            return;
        };
        let Some(regs) = lp::getregs(pid) else {
            return;
        };
        let ret = lp::retval(&regs);
        let allowed = ret >= 0;
        let errno = if allowed { None } else { Some(-ret as i32) };
        match p.kind {
            PendingKind::File { path, op } if !path.is_empty() => {
                self.push_file(&path, op, allowed, errno)
            }
            PendingKind::Net { addr, family } => self.push_net(&addr, family, allowed),
            PendingKind::Exec { argv0, argv_preview } if allowed && !argv0.is_empty() => {
                self.push_exec(argv0, argv_preview)
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod collector_tests {
    use super::*;
    use trace::FileOp;

    #[test]
    fn dedup_and_cap() {
        let mut c = EventCollector::new_for_test(vec!["/work".into()]);
        for _ in 0..3 {
            c.push_file("/work/a", FileOp::Read, true, None);
        }
        for i in 0..(TRACE_EVENT_CAP + 10) {
            c.push_file(&format!("/p{i}"), FileOp::Stat, false, Some(13));
        }
        let t = c.finish();
        assert_eq!(t.files.iter().find(|f| f.path == "/work/a").unwrap().count, 3);
        assert!(t.truncated);
        assert!(t.files.len() <= TRACE_EVENT_CAP + 1);
    }
}
