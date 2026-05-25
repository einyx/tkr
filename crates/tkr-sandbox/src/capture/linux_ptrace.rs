use crate::capture::trace::{CaptureKind, SandboxTrace};
use crate::capture::{verdict, CaptureBackend, EventCollector};
use crate::error::SandboxError;
use crate::exec::SandboxOutput;
use crate::policy::SandboxPolicy;
use nix::sys::ptrace;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{fork, ForkResult, Pid};
use std::collections::HashMap;
use std::ffi::CString;

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
    trace.verdict = verdict::compute_verdict(&trace.files, &trace.net, &writable);
    trace.capture_kind = CaptureKind::Full;

    let out = SandboxOutput {
        stdout: Vec::new(),
        stderr: Vec::new(),
        exit: exit_code,
        truncated: false,
    };
    Ok((out, trace))
}
