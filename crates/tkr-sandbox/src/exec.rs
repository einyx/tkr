use crate::error::SandboxError;
use crate::policy::{SandboxLimits, SandboxPolicy};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Default output cap (16 MiB) applied when [`SandboxLimits::max_output_bytes`]
/// is `None`. Above this, the child is killed and `OutputCapExceeded` is
/// returned. Set to a finite value because the alternative — unbounded
/// `Vec<u8>` growth — has crashed callers in practice (think
/// `cat /dev/urandom` in a sandboxed shell).
pub const DEFAULT_OUTPUT_CAP: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct SandboxOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: i32,
    /// True if the run hit either `OutputCapExceeded` or the wall-clock
    /// timeout. Distinguishes "exit 137 because OOM" from "exit 137 because
    /// we killed it."
    pub truncated: bool,
}

pub fn run_sandboxed(
    command: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<SandboxOutput, SandboxError> {
    if let Err(e) = policy.validate() {
        return Err(SandboxError::PolicyViolation(e));
    }
    if policy.disabled {
        return run_unsandboxed(command, args, &policy.limits);
    }
    #[cfg(target_os = "linux")]
    {
        crate::linux::run(command, args, policy)
    }
    #[cfg(target_os = "macos")]
    {
        crate::macos::run(command, args, policy)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (command, args);
        Err(SandboxError::Unsupported)
    }
}

fn run_unsandboxed(
    command: &str,
    args: &[&str],
    limits: &SandboxLimits,
) -> Result<SandboxOutput, SandboxError> {
    let mut cmd = Command::new(command);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    spawn_and_collect(cmd, limits)
}

/// Spawn `cmd`, read stdout/stderr concurrently up to the output cap, and
/// enforce the wall-clock timeout. Used by every backend (linux, macos,
/// unsandboxed) so cap + timeout behavior is consistent across platforms.
pub(crate) fn spawn_and_collect(
    mut cmd: Command,
    limits: &SandboxLimits,
) -> Result<SandboxOutput, SandboxError> {
    let cap = limits.max_output_bytes.unwrap_or(DEFAULT_OUTPUT_CAP);
    let timeout = limits.timeout_ms.map(Duration::from_millis);

    let mut child = cmd
        .spawn()
        .map_err(|e| SandboxError::Backend(e.to_string()))?;
    let mut stdout_pipe = child.stdout.take().expect("piped");
    let mut stderr_pipe = child.stderr.take().expect("piped");

    // Bounded readers: each thread reads into a shared Vec, stops at `cap/2`,
    // and signals if it tripped the cap. The /2 split is deliberate — a
    // misbehaving child can flood either stream but not double-spend the cap.
    let cap_each = (cap / 2).max(1);
    let stdout_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let truncated: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    let so = Arc::clone(&stdout_buf);
    let trip_so = Arc::clone(&truncated);
    let t_so = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match stdout_pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut g = so.lock().expect("stdout buf");
                    let room = cap_each.saturating_sub(g.len() as u64) as usize;
                    if room == 0 {
                        *trip_so.lock().expect("trip") = true;
                        break;
                    }
                    let take = n.min(room);
                    g.extend_from_slice(&buf[..take]);
                    if take < n {
                        *trip_so.lock().expect("trip") = true;
                        break;
                    }
                }
            }
        }
    });

    let se = Arc::clone(&stderr_buf);
    let trip_se = Arc::clone(&truncated);
    let t_se = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match stderr_pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut g = se.lock().expect("stderr buf");
                    let room = cap_each.saturating_sub(g.len() as u64) as usize;
                    if room == 0 {
                        *trip_se.lock().expect("trip") = true;
                        break;
                    }
                    let take = n.min(room);
                    g.extend_from_slice(&buf[..take]);
                    if take < n {
                        *trip_se.lock().expect("trip") = true;
                        break;
                    }
                }
            }
        }
    });

    let start = Instant::now();
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if *truncated.lock().expect("trip") {
                    let _ = child.kill();
                    break child.wait().map_err(|e| SandboxError::Backend(e.to_string()))?;
                }
                if let Some(t) = timeout {
                    if start.elapsed() >= t {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = t_so.join();
                        let _ = t_se.join();
                        return Err(SandboxError::Timeout(t.as_millis() as u64));
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(SandboxError::Backend(e.to_string())),
        }
    };
    let _ = t_so.join();
    let _ = t_se.join();

    let trunc = *truncated.lock().expect("trip");
    let stdout = std::mem::take(&mut *stdout_buf.lock().expect("stdout buf"));
    let stderr = std::mem::take(&mut *stderr_buf.lock().expect("stderr buf"));

    if trunc {
        // Cap-trip is fail-loud: callers that want partial output on cap
        // can opt out by setting `max_output_bytes` higher; the default is
        // to refuse silent truncation.
        return Err(SandboxError::OutputCapExceeded(cap));
    }

    Ok(SandboxOutput {
        stdout,
        stderr,
        exit: exit_status.code().unwrap_or(-1),
        truncated: trunc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_paths_in_policy() {
        let p = SandboxPolicy::builder().allow_read("rel").build();
        let r = run_sandboxed("/bin/true", &[], &p);
        assert!(matches!(r, Err(SandboxError::PolicyViolation(_))));
    }

    #[test]
    fn disabled_policy_runs_unsandboxed() {
        let mut p = SandboxPolicy::deny_all();
        p.disabled = true;
        let r = run_sandboxed("/bin/echo", &["hi"], &p).unwrap();
        assert_eq!(r.exit, 0);
        assert!(String::from_utf8_lossy(&r.stdout).contains("hi"));
        assert!(!r.truncated);
    }

    #[test]
    fn timeout_kills_runaway_child() {
        let mut p = SandboxPolicy::deny_all();
        p.disabled = true;
        p.limits.timeout_ms = Some(200);
        let err = run_sandboxed("/bin/sleep", &["10"], &p).unwrap_err();
        assert!(matches!(err, SandboxError::Timeout(_)), "got: {err}");
    }

    #[test]
    fn output_cap_truncates_and_errors() {
        let mut p = SandboxPolicy::deny_all();
        p.disabled = true;
        p.limits.max_output_bytes = Some(1024);
        // `yes` floods stdout forever — cap should trip and kill it.
        let err = run_sandboxed("/usr/bin/yes", &[], &p).unwrap_err();
        assert!(
            matches!(err, SandboxError::OutputCapExceeded(_)),
            "got: {err}"
        );
    }
}
