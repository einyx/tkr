use crate::error::SandboxError;
use crate::exec::SandboxOutput;
use crate::policy::SandboxPolicy;
use std::os::unix::process::CommandExt;
use std::process::Command;

pub fn run(
    command: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<SandboxOutput, SandboxError> {
    let read = policy.fs_read.clone();
    let write = policy.fs_write.clone();

    let mut cmd = Command::new(command);
    cmd.args(args);

    unsafe {
        cmd.pre_exec(move || {
            apply_landlock(&read, &write).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("landlock: {}", e))
            })
        });
    }

    let out = cmd
        .output()
        .map_err(|e| SandboxError::Backend(e.to_string()))?;
    Ok(SandboxOutput {
        stdout: out.stdout,
        stderr: out.stderr,
        exit: out.status.code().unwrap_or(-1),
    })
}

fn apply_landlock(
    fs_read: &[std::path::PathBuf],
    fs_write: &[std::path::PathBuf],
) -> Result<(), String> {
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, ABI,
    };

    let abi = ABI::V2;
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| format!("handle_access: {}", e))?
        .create()
        .map_err(|e| format!("create: {}", e))?;

    for p in fs_read {
        let fd = PathFd::new(p).map_err(|e| format!("open {}: {}", p.display(), e))?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, AccessFs::from_read(abi)))
            .map_err(|e| format!("add_rule read {}: {}", p.display(), e))?;
    }
    for p in fs_write {
        let fd = PathFd::new(p).map_err(|e| format!("open {}: {}", p.display(), e))?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, AccessFs::from_all(abi)))
            .map_err(|e| format!("add_rule write {}: {}", p.display(), e))?;
    }

    let status = ruleset
        .restrict_self()
        .map_err(|e| format!("restrict_self: {}", e))?;

    if matches!(status.ruleset, RulesetStatus::NotEnforced) {
        return Err("landlock not supported by kernel".into());
    }
    Ok(())
}
