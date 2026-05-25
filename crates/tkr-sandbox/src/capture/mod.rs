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
