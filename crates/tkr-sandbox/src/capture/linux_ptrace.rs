use crate::capture::trace::SandboxTrace;
use crate::capture::CaptureBackend;
use crate::error::SandboxError;
use crate::exec::SandboxOutput;
use crate::policy::SandboxPolicy;

pub struct LinuxPtraceBackend;

impl CaptureBackend for LinuxPtraceBackend {
    fn run(&self, command: &str, args: &[&str], policy: &SandboxPolicy)
        -> Result<(SandboxOutput, SandboxTrace), SandboxError> {
        // Temporary: real ptrace lands in a later task.
        let out = crate::exec::run_sandboxed_output_only(command, args, policy)?;
        Ok((out, SandboxTrace::none()))
    }
}
