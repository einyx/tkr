use crate::error::SandboxError;
use crate::exec::{spawn_and_collect, SandboxOutput};
use crate::policy::{NetworkPolicy, SandboxPolicy};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

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
    // Mirror the linux backend: clear the parent env, then re-add only the
    // names listed in policy.env_allow. Always forward PATH so the kernel
    // can resolve `command` when not absolute.
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
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    spawn_and_collect(cmd, &policy.limits)
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
    // Network. The base profile starts with `(deny default)`, so unless
    // we explicitly emit `(allow network*)` the agent has no network at
    // all on macOS — that's the silent bug on Mac today. Match the
    // documented per-mode contract:
    //   * Inherit  → unrestricted (allow network*), same as a normal
    //                shell. Required for Claude Code to reach Anthropic.
    //   * DenyAll  → no allow rule. The (deny default) already covers
    //                it; the explicit comment marker is for SBPL
    //                debugging via `sandbox-exec -p`.
    //   * Allow    → grant exact ports for connect/bind, nothing else.
    //                DNS goes through mDNSResponder via mach-lookup
    //                (already allowed in the preamble), so port 53/UDP
    //                isn't needed.
    match &policy.limits.network {
        NetworkPolicy::Inherit => {
            s.push_str("(allow network*)\n");
        }
        NetworkPolicy::DenyAll => {
            s.push_str("; network: deny-all (handled by `deny default`)\n");
        }
        NetworkPolicy::Allow { connect_ports, bind_ports } => {
            for port in connect_ports {
                s.push_str(&format!(
                    "(allow network-outbound (remote tcp \"*:{port}\"))\n"
                ));
            }
            for port in bind_ports {
                s.push_str(&format!(
                    "(allow network-inbound (local tcp \"*:{port}\"))\n"
                ));
            }
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
    #[test]
    fn network_inherit_allows_all_network() {
        // Default policy is Inherit. Without this rule the `(deny default)`
        // at the top silently blocks the agent from reaching Anthropic.
        let s = build_profile(&SandboxPolicy::default());
        assert!(s.contains("(allow network*)"));
    }
    #[test]
    fn network_deny_all_emits_no_allow_rule() {
        let p = SandboxPolicy::builder().deny_network().build();
        let s = build_profile(&p);
        assert!(!s.contains("(allow network"));
    }
    #[test]
    fn network_allow_ports_emits_per_port_rules() {
        let p = SandboxPolicy::builder()
            .allow_tcp_connect(443)
            .allow_tcp_bind(8080)
            .build();
        let s = build_profile(&p);
        assert!(s.contains("(allow network-outbound (remote tcp \"*:443\"))"));
        assert!(s.contains("(allow network-inbound (local tcp \"*:8080\"))"));
        // Must NOT include a broad allow when a specific allowlist is set.
        assert!(!s.contains("(allow network*)"));
    }
}
