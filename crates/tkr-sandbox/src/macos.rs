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

    // --- Baseline allows for any binary to reach main() ---
    //
    // `(deny default)` on macOS is more aggressive than its name suggests:
    // it blocks mach traps, process-info self-introspection, file-read-metadata
    // (stat), file-map-executable (dyld dylib load), and POSIX shm — all of
    // which dyld + libsystem touch *before* the child's own `main` runs.
    // Without these, sandbox-exec SIGKILLs the child during image load and
    // emits its denial only to the kernel log, so the parent sees a silent
    // exit with empty stdout/stderr (v0.3.0 bug).
    //
    // Drawn from Apple's open-sourced profiles in
    //   /System/Library/Sandbox/Profiles/{bsd,system}.sb
    // and from WebKit's `com.apple.WebKit.WebContent.sb`.

    s.push_str("(allow process-fork process-exec)\n");
    s.push_str("(allow process-info* (target self))\n");
    s.push_str("(allow signal (target self))\n");
    s.push_str("(allow mach-lookup)\n");
    s.push_str("(allow mach-priv-host-port)\n");
    s.push_str("(allow mach-register)\n");
    s.push_str("(allow sysctl-read)\n");
    s.push_str("(allow ipc-posix-shm)\n");
    s.push_str("(allow ipc-posix-sem)\n");
    // stat()/getattr on arbitrary paths — dyld and libsystem need this to
    // even decide whether a dylib exists before reading it.
    s.push_str("(allow file-read-metadata)\n");
    // mmap PROT_EXEC of a readable file — dyld loads dylibs this way.
    s.push_str("(allow file-map-executable)\n");
    // ioctls on inherited tty/pty fds. Pipe stdio doesn't touch this, but
    // `tkr sandbox claude` is interactive and will inherit tty fds.
    s.push_str("(allow file-ioctl)\n");

    // /dev nodes every binary touches at startup.
    s.push_str(
        "(allow file-read* \
         (literal \"/dev/null\") \
         (literal \"/dev/zero\") \
         (literal \"/dev/random\") \
         (literal \"/dev/urandom\") \
         (literal \"/dev/autofs_nowait\") \
         (literal \"/dev/dtracehelper\"))\n",
    );
    s.push_str(
        "(allow file-write* \
         (literal \"/dev/null\") \
         (literal \"/dev/dtracehelper\"))\n",
    );

    // System paths the child needs to load its binary + dylibs. These are
    // redundant with the policy.fs_read entries that `cmds/sandbox.rs`
    // injects on `--system=true` (the default), but we keep them so a
    // policy built directly via the library API (no CLI defaults) still
    // gets a runnable child.
    s.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
    s.push_str("(allow file-read* (subpath \"/usr/bin\"))\n");
    s.push_str("(allow file-read* (subpath \"/bin\"))\n");
    s.push_str("(allow file-read* (subpath \"/System/Library\"))\n");
    s.push_str("(allow file-read* (subpath \"/Library/Apple/System\"))\n");
    s.push_str("(allow file-read* (subpath \"/private/var/folders\"))\n");
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
    fn profile_has_baseline_allows_for_dyld() {
        // Regression: v0.3.0 shipped without these and every sandboxed
        // child was SIGKILLed during image load (silent exit, empty stdout).
        let s = build_profile(&SandboxPolicy::default());
        for needle in [
            "(allow file-read-metadata)",
            "(allow file-map-executable)",
            "(allow ipc-posix-shm)",
            "(allow process-info* (target self))",
            "(allow mach-priv-host-port)",
        ] {
            assert!(s.contains(needle), "profile missing baseline allow: {needle}");
        }
    }
    // End-to-end: actually invoke sandbox-exec and assert the child runs
    // and its stdout reaches us. The unit-level `profile_*` tests above
    // only check string contents; they would have happily passed even
    // for the broken v0.3.0 profile. Only this test catches "child dies
    // silently mid-dyld."
    #[test]
    #[cfg(target_os = "macos")]
    fn echo_runs_under_default_sandbox() {
        use crate::exec::run_sandboxed;
        use crate::policy::SandboxPolicy;
        let policy = SandboxPolicy::builder()
            .allow_read("/usr/lib")
            .allow_read("/usr/bin")
            .allow_read("/bin")
            .allow_read("/System")
            .build();
        let out = run_sandboxed("/bin/echo", &["hi"], &policy)
            .expect("sandbox-exec should not error");
        assert_eq!(out.exit, 0, "stderr was: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "hi",
            "stdout empty — profile likely SIGKILLed child during dyld load"
        );
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
