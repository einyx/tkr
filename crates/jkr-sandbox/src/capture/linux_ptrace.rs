use crate::capture::trace::{CaptureKind, SandboxTrace};
use crate::capture::{CaptureBackend, EventCollector};
use crate::error::SandboxError;
use crate::exec::SandboxOutput;
use crate::policy::SandboxPolicy;
use nix::sys::ptrace;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{fork, ForkResult, Pid};
use std::collections::HashMap;
use std::ffi::CString;

#[cfg(target_arch = "x86_64")]
pub(crate) fn syscall_nr(regs: &libc::user_regs_struct) -> u64 {
    regs.orig_rax
}
#[cfg(target_arch = "x86_64")]
pub(crate) fn arg(regs: &libc::user_regs_struct, i: usize) -> u64 {
    [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9][i]
}
#[cfg(target_arch = "x86_64")]
pub(crate) fn retval(regs: &libc::user_regs_struct) -> i64 {
    regs.rax as i64
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn syscall_nr(regs: &libc::user_regs_struct) -> u64 {
    regs.regs[8]
}
#[cfg(target_arch = "aarch64")]
pub(crate) fn arg(regs: &libc::user_regs_struct, i: usize) -> u64 {
    regs.regs[i]
}
#[cfg(target_arch = "aarch64")]
pub(crate) fn retval(regs: &libc::user_regs_struct) -> i64 {
    regs.regs[0] as i64
}

pub(crate) fn getregs(pid: Pid) -> Option<libc::user_regs_struct> {
    ptrace::getregs(pid).ok()
}

/// Read a NUL-terminated string from the tracee's address space, bounded.
pub(crate) fn read_cstr(pid: Pid, addr: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    if addr == 0 {
        return String::new();
    }
    let path = format!("/proc/{}/mem", pid.as_raw());
    let mut buf = [0u8; 4096];
    if let Ok(mut f) = std::fs::File::open(&path) {
        if f.seek(SeekFrom::Start(addr)).is_ok() {
            if let Ok(n) = f.read(&mut buf) {
                let end = buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
                return String::from_utf8_lossy(&buf[..end]).into_owned();
            }
        }
    }
    String::new()
}

/// Decode a sockaddr at `addr` into ("ip:port", family). Reads family first.
pub(crate) fn read_sockaddr(
    pid: Pid,
    addr: u64,
) -> Option<(String, crate::capture::trace::NetFamily)> {
    use crate::capture::trace::NetFamily;
    use std::io::{Read, Seek, SeekFrom};
    if addr == 0 {
        return None;
    }
    let path = format!("/proc/{}/mem", pid.as_raw());
    let mut f = std::fs::File::open(&path).ok()?;
    f.seek(SeekFrom::Start(addr)).ok()?;
    let mut hdr = [0u8; 28]; // sockaddr_in6 sized
    let n = f.read(&mut hdr).ok()?;
    if n < 2 {
        return None;
    }
    let fam = u16::from_ne_bytes([hdr[0], hdr[1]]) as i32;
    match fam {
        libc::AF_INET => {
            let port = u16::from_be_bytes([hdr[2], hdr[3]]);
            let ip = std::net::Ipv4Addr::new(hdr[4], hdr[5], hdr[6], hdr[7]);
            Some((format!("{ip}:{port}"), NetFamily::V4))
        }
        libc::AF_INET6 => {
            let port = u16::from_be_bytes([hdr[2], hdr[3]]);
            let mut o = [0u8; 16];
            o.copy_from_slice(&hdr[8..24]);
            let ip = std::net::Ipv6Addr::from(o);
            Some((format!("[{ip}]:{port}"), NetFamily::V6))
        }
        libc::AF_UNIX => Some(("unix".into(), NetFamily::Unix)),
        _ => None,
    }
}

pub struct LinuxPtraceBackend;

impl CaptureBackend for LinuxPtraceBackend {
    fn run(
        &self,
        command: &str,
        args: &[&str],
        policy: &SandboxPolicy,
    ) -> Result<(SandboxOutput, SandboxTrace), SandboxError> {
        if let Err(e) = policy.validate() {
            return Err(SandboxError::PolicyViolation(e));
        }
        if policy.disabled {
            let out = crate::exec::run_sandboxed_output_only(command, args, policy)?;
            return Ok((out, SandboxTrace::none()));
        }
        match unsafe { fork() }.map_err(|e| SandboxError::Backend(e.to_string()))? {
            ForkResult::Child => {
                child_setup_and_exec(command, args, policy);
            }
            ForkResult::Parent { child } => run_tracer(child, policy),
        }
    }
}

fn child_setup_and_exec(command: &str, args: &[&str], policy: &SandboxPolicy) -> ! {
    let _ = crate::linux::apply_rlimits(&policy.limits);
    let _ = crate::linux::apply_landlock_full(
        &policy.fs_read,
        &policy.fs_write,
        &policy.limits.network,
    );
    let _ = ptrace::traceme();
    let _ = nix::sys::signal::raise(Signal::SIGSTOP);
    let c_cmd = CString::new(command).unwrap();
    let mut c_args = vec![CString::new(command).unwrap()];
    for a in args {
        c_args.push(CString::new(*a).unwrap());
    }
    let _ = nix::unistd::execvp(&c_cmd, &c_args);
    unsafe { libc::_exit(127) }
}

struct TraceeState {
    at_entry: bool,
}

fn run_tracer(
    root: Pid,
    policy: &SandboxPolicy,
) -> Result<(SandboxOutput, SandboxTrace), SandboxError> {
    // Wait for the child's initial SIGSTOP (from raise() above) before
    // setting options — the tracee must be stopped and attached.
    waitpid(root, None).map_err(|e| SandboxError::Backend(e.to_string()))?;
    let opts = ptrace::Options::PTRACE_O_TRACESYSGOOD
        | ptrace::Options::PTRACE_O_TRACEFORK
        | ptrace::Options::PTRACE_O_TRACEVFORK
        | ptrace::Options::PTRACE_O_TRACECLONE
        | ptrace::Options::PTRACE_O_TRACEEXEC;
    ptrace::setoptions(root, opts).map_err(|e| SandboxError::Backend(e.to_string()))?;

    let mut states: HashMap<Pid, TraceeState> = HashMap::new();
    states.insert(root, TraceeState { at_entry: true });

    let writable: Vec<String> = policy
        .fs_write
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let mut collector = EventCollector::new(writable.clone());

    let _ = ptrace::syscall(root, None);
    let mut exit_code: i32 = -1;

    loop {
        let status = match waitpid(None, None) {
            Ok(s) => s,
            Err(nix::errno::Errno::ECHILD) => break,
            Err(e) => return Err(SandboxError::Backend(e.to_string())),
        };
        match status {
            WaitStatus::PtraceSyscall(pid) => {
                let st = states
                    .entry(pid)
                    .or_insert(TraceeState { at_entry: true });
                if st.at_entry {
                    collector.on_entry(pid);
                } else {
                    collector.on_exit(pid);
                }
                st.at_entry = !st.at_entry;
                let _ = ptrace::syscall(pid, None);
            }
            WaitStatus::PtraceEvent(pid, _, _) => {
                // PTRACE_EVENT_EXEC resets the tracee's registers but does NOT flip at_entry, so entry/exit pairing may be stale for that pid after exec.
                let _ = ptrace::syscall(pid, None);
            }
            WaitStatus::Exited(pid, code) => {
                states.remove(&pid);
                if pid == root {
                    exit_code = code;
                }
                if states.is_empty() {
                    break;
                }
            }
            WaitStatus::Signaled(pid, _, _) => {
                states.remove(&pid);
                if states.is_empty() {
                    break;
                }
            }
            WaitStatus::Stopped(pid, sig) => {
                // A newly-attached child (from fork/clone) arrives via a group
                // stop with SIGSTOP — that signal must NOT be re-delivered or
                // the child stays stopped forever (hang). Swallow SIGSTOP and
                // SIGTRAP; forward any genuine signal to the tracee.
                let deliver = if sig == Signal::SIGTRAP || sig == Signal::SIGSTOP {
                    None
                } else {
                    Some(sig)
                };
                let _ = ptrace::syscall(pid, deliver);
            }
            _ => {}
        }
    }

    let mut trace = collector.finish();
    trace.capture_kind = CaptureKind::Full;

    let out = SandboxOutput {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit: exit_code,
        truncated: false,
    };
    Ok((out, trace))
}
