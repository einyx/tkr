use crate::error::SandboxError;
use crate::exec::{spawn_and_collect, SandboxOutput};
use crate::policy::SandboxPolicy;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

pub fn run(
    command: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<SandboxOutput, SandboxError> {
    let read = policy.fs_read.clone();
    let write = policy.fs_write.clone();
    let limits = policy.limits.clone();

    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Strip the parent environment unconditionally, then re-add only the
    // names listed in policy.env_allow (looking up live values at spawn).
    // PATH is always forwarded so the kernel can resolve `command` when
    // it is not an absolute path; refusing it would silently break every
    // policy that does not think to allowlist it.
    cmd.env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    for name in &policy.env_allow {
        if name == "PATH" {
            continue;
        }
        if let Ok(value) = std::env::var(name) {
            cmd.env(name, value);
        }
    }

    unsafe {
        cmd.pre_exec(move || {
            // Order matters: rlimits before landlock so a misconfigured
            // rlimit fails fast with a clear errno, not a confused landlock
            // status downstream.
            apply_rlimits(&limits).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("rlimit: {e}"))
            })?;
            apply_landlock_full(&read, &write, &limits.network).map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("landlock: {e}"))
            })
        });
    }

    spawn_and_collect(cmd, &policy.limits)
}

fn apply_rlimits(limits: &crate::policy::SandboxLimits) -> Result<(), String> {
    use rlimit::{setrlimit, Resource};
    if let Some(n) = limits.memory_bytes {
        setrlimit(Resource::AS, n, n).map_err(|e| format!("AS: {e}"))?;
    }
    if let Some(n) = limits.cpu_seconds {
        setrlimit(Resource::CPU, n, n).map_err(|e| format!("CPU: {e}"))?;
    }
    if let Some(n) = limits.file_size_bytes {
        setrlimit(Resource::FSIZE, n, n).map_err(|e| format!("FSIZE: {e}"))?;
    }
    Ok(())
}

fn apply_landlock_full(
    fs_read: &[std::path::PathBuf],
    fs_write: &[std::path::PathBuf],
    net: &crate::policy::NetworkPolicy,
) -> Result<(), String> {
    use crate::policy::NetworkPolicy;
    use landlock::{
        Access, AccessFs, AccessNet, NetPort, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus, ABI,
    };

    // Network rules need ABI v4 (Linux 6.7+); fs rules work from v2.
    // We always request v4 so net rules attach when the kernel supports them.
    let abi = ABI::V4;
    let needs_net = !matches!(net, NetworkPolicy::Inherit);

    let mut builder = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| format!("handle_access fs: {e}"))?;
    if needs_net {
        builder = builder
            .handle_access(AccessNet::ConnectTcp | AccessNet::BindTcp)
            .map_err(|e| format!("handle_access net: {e}"))?;
    }
    let mut ruleset = builder.create().map_err(|e| format!("create: {e}"))?;

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

    match net {
        NetworkPolicy::Inherit => {}
        NetworkPolicy::DenyAll => {
            // handle_access on Connect+Bind with no rules added = block all.
        }
        NetworkPolicy::Allow {
            connect_ports,
            bind_ports,
        } => {
            for port in connect_ports {
                ruleset = ruleset
                    .add_rule(NetPort::new(*port, AccessNet::ConnectTcp))
                    .map_err(|e| format!("add_rule connect :{port}: {e}"))?;
            }
            for port in bind_ports {
                ruleset = ruleset
                    .add_rule(NetPort::new(*port, AccessNet::BindTcp))
                    .map_err(|e| format!("add_rule bind :{port}: {e}"))?;
            }
        }
    }

    let status = ruleset
        .restrict_self()
        .map_err(|e| format!("restrict_self: {e}"))?;

    if matches!(status.ruleset, RulesetStatus::NotEnforced) {
        return Err("landlock not supported by kernel".into());
    }
    Ok(())
}
