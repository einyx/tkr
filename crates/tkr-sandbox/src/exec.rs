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
    }
}
