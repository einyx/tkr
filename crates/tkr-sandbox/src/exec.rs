use crate::error::SandboxError;
use crate::policy::SandboxPolicy;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct SandboxOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: i32,
}

pub fn run_sandboxed(
    command: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<SandboxOutput, SandboxError> {
    if let Err(e) = policy.validate() {
        return Err(SandboxError::PolicyViolation(e));
    }
    // Tasks 5/6 will replace this with platform-specific dispatch.
    run_unsandboxed(command, args)
}

fn run_unsandboxed(command: &str, args: &[&str]) -> Result<SandboxOutput, SandboxError> {
    let out = Command::new(command).args(args).output()?;
    Ok(SandboxOutput {
        stdout: out.stdout, stderr: out.stderr,
        exit: out.status.code().unwrap_or(-1),
    })
}
