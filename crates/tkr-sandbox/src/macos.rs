use crate::error::SandboxError;
use crate::exec::SandboxOutput;
use crate::policy::SandboxPolicy;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

pub fn run(
    command: &str,
    args: &[&str],
    policy: &SandboxPolicy,
) -> Result<SandboxOutput, SandboxError> {
    let profile = build_profile(policy);
    let mut tmp =
        tempfile::NamedTempFile::new().map_err(|e| SandboxError::Backend(e.to_string()))?;
    tmp.write_all(profile.as_bytes())
        .map_err(|e| SandboxError::Backend(e.to_string()))?;
    let profile_path = tmp.path().to_path_buf();

    let mut cmd = Command::new("/usr/bin/sandbox-exec");
    cmd.arg("-f").arg(&profile_path).arg(command).args(args);
    let out = cmd
        .output()
        .map_err(|e| SandboxError::Backend(e.to_string()))?;
    Ok(SandboxOutput {
        stdout: out.stdout,
        stderr: out.stderr,
        exit: out.status.code().unwrap_or(-1),
    })
}

/// Resolve a path to its canonical form, falling back to the original if canonicalization fails.
fn canonical(p: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

pub(crate) fn build_profile(policy: &SandboxPolicy) -> String {
    let mut s = String::from("(version 1)\n(deny default)\n");
    s.push_str("(allow process-fork process-exec)\n");
    s.push_str("(allow mach-lookup)\n");
    s.push_str("(allow sysctl-read)\n");
    s.push_str("(allow signal (target self))\n");
    s.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
    s.push_str("(allow file-read* (subpath \"/usr/bin\"))\n");
    s.push_str("(allow file-read* (subpath \"/System/Library\"))\n");
    s.push_str("(allow file-read* (subpath \"/Library/Apple/System\"))\n");
    s.push_str("(allow file-read* (subpath \"/private/var/folders\"))\n");
    s.push_str("(allow file-read* (literal \"/dev/null\") (literal \"/dev/urandom\"))\n");
    s.push_str("(allow file-write* (literal \"/dev/null\"))\n");
    for p in &policy.fs_read {
        let canon = canonical(p);
        let path_str = canon.to_string_lossy();
        let esc_path = esc(&path_str);
        s.push_str(&format!("(allow file-read* (subpath \"{esc_path}\"))\n"));
        // Also allow the original path in case they differ
        if canon != *p {
            let esc_orig = esc(&p.to_string_lossy());
            s.push_str(&format!("(allow file-read* (subpath \"{esc_orig}\"))\n"));
        }
    }
    for p in &policy.fs_write {
        let canon = canonical(p);
        let canon_str = canon.to_string_lossy();
        let e = esc(&canon_str);
        s.push_str(&format!("(allow file-read* (subpath \"{e}\"))\n"));
        s.push_str(&format!("(allow file-write* (subpath \"{e}\"))\n"));
        // Also allow the original path
        if canon != *p {
            let orig = esc(&p.to_string_lossy());
            s.push_str(&format!("(allow file-read* (subpath \"{orig}\"))\n"));
            s.push_str(&format!("(allow file-write* (subpath \"{orig}\"))\n"));
        }
    }
    s
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_contains_deny_default() {
        let s = build_profile(&SandboxPolicy::default());
        assert!(s.contains("(deny default)"));
    }
    #[test]
    fn profile_includes_writable_paths() {
        let p = SandboxPolicy::builder().allow_write("/tmp/foo").build();
        let s = build_profile(&p);
        assert!(
            s.contains("(allow file-write* (subpath \"/tmp/foo\"))")
                || s.contains("(allow file-write* (subpath \"/private/tmp/foo\"))")
        );
    }
}
