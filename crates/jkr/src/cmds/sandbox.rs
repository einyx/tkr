//! `jkr sandbox run` — execute a command under jkr-sandbox.
//!
//! Defaults are deliberately strict: deny-all fs, empty env, 16 MiB output cap,
//! no timeout. Operators opt in to fs/env/resource grants via flags.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Instant;
use jkr_sandbox::capture::trace::{CaptureKind, SandboxTrace};
use jkr_sandbox::exec::run_sandboxed_output_only;
use jkr_sandbox::{
    run_sandboxed, run_sandboxed_interactive, SandboxError, SandboxPolicy, VerdictLevel,
};

#[allow(clippy::too_many_arguments)]
pub fn run(
    system: bool,
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    env: Vec<String>,
    memory: Option<u64>,
    cpu: Option<u64>,
    timeout_ms: Option<u64>,
    max_output: Option<u64>,
    no_network: bool,
    allow_connect: Vec<u16>,
    allow_bind: Vec<u16>,
    trace: bool,
    trace_json: bool,
    interactive: bool,
    argv: Vec<String>,
) -> Result<()> {
    let (cmd, args) = argv
        .split_first()
        .map(|(c, rest)| (c.clone(), rest.to_vec()))
        .context("sandbox: missing command")?;

    // System paths the child needs just to exec the binary you asked
    // for: the loader, libc, and the binary's own directory tree. We
    // only add paths that exist (harmless cross-platform — /lib64
    // doesn't exist on macOS or musl Linux, etc.). Users who really
    // want deny-everything pass `--system=false`.
    let mut effective_read: Vec<PathBuf> = Vec::new();
    if system {
        for p in [
            "/bin", "/usr", "/lib", "/lib64", "/etc",
            // macOS exec paths — harmless when absent on Linux.
            "/opt", "/System", "/Library", "/private",
        ] {
            let pb = PathBuf::from(p);
            if pb.exists() {
                effective_read.push(pb);
            }
        }
    }
    effective_read.extend(read);

    let mut builder = SandboxPolicy::builder();
    for p in effective_read {
        builder = builder.allow_read(p);
    }
    for p in write {
        builder = builder.allow_write(p);
    }
    for e in env {
        builder = builder.allow_env(e);
    }
    if let Some(n) = memory {
        builder = builder.memory_bytes(n);
    }
    if let Some(n) = cpu {
        builder = builder.cpu_seconds(n);
    }
    if let Some(n) = timeout_ms {
        builder = builder.timeout_ms(n);
    }
    if let Some(n) = max_output {
        builder = builder.max_output_bytes(n);
    }
    if no_network {
        builder = builder.deny_network();
    }
    for p in allow_connect {
        builder = builder.allow_tcp_connect(p);
    }
    for p in allow_bind {
        builder = builder.allow_tcp_bind(p);
    }
    let policy = builder.build();

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let started = Instant::now();
    // Three execution modes:
    //   interactive  -> Landlock-only, inherited tty (agent TUIs). No ptrace.
    //   trace flags  -> ptrace capture backend (the only path that builds a
    //                   real SandboxTrace; ~3.5x slower, so opt-in).
    //   default      -> Landlock-only, captured+printed output. No ptrace.
    let result = if interactive {
        run_sandboxed_interactive(&cmd, &arg_refs, &policy).map(|o| (o, SandboxTrace::none()))
    } else if trace || trace_json {
        run_sandboxed(&cmd, &arg_refs, &policy)
    } else {
        run_sandboxed_output_only(&cmd, &arg_refs, &policy).map(|o| (o, SandboxTrace::none()))
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok((out, sbx_trace)) => {
            // Pass-through stdout/stderr, then propagate the child's exit code.
            use std::io::Write;
            let _ = std::io::stdout().write_all(&out.stdout);
            let _ = std::io::stderr().write_all(&out.stderr);
            // Behavioral trace goes to stderr only: on the Linux ptrace path the
            // child's real stdout is streamed live, so anything on stdout here
            // would corrupt a downstream pipe.
            if trace_json {
                match serde_json::to_string(&sbx_trace) {
                    Ok(j) => eprintln!("{j}"),
                    Err(e) => eprintln!("jkr sandbox: trace serialize failed: {e}"),
                }
            } else if trace {
                eprint!("{}", format_trace_summary(&sbx_trace));
            }
            emit_ingest(&cmd, out.exit, false, duration_ms);
            std::process::exit(out.exit);
        }
        Err(SandboxError::Timeout(ms)) => {
            eprintln!("jkr sandbox: timeout after {ms}ms");
            emit_ingest(&cmd, 124, false, duration_ms);
            std::process::exit(124); // GNU timeout convention.
        }
        Err(SandboxError::OutputCapExceeded(n)) => {
            eprintln!("jkr sandbox: child exceeded {n}-byte output cap");
            emit_ingest(&cmd, 125, true, duration_ms);
            std::process::exit(125);
        }
        Err(e) => {
            emit_ingest(&cmd, 1, false, duration_ms);
            // The most common first-time failure is "Permission denied"
            // because the binary's own directory isn't in --read. Surface
            // that hint inline so users don't have to grep docs.
            let msg = e.to_string();
            if msg.contains("Permission denied") || msg.contains("os error 13") {
                eprintln!(
                    "jkr sandbox: {msg}\n\
                     hint: the child couldn't exec `{cmd}`. \
                     If you passed `--system=false`, make sure --read \
                     includes the binary's directory + /lib + /lib64 + /etc."
                );
            }
            Err(e.into())
        }
    }
}

fn level_str(level: &VerdictLevel) -> &'static str {
    match level {
        VerdictLevel::Clean => "clean",
        VerdictLevel::Notable => "notable",
        VerdictLevel::Suspicious => "suspicious",
    }
}

/// Render a token-tight, human-readable trace summary (house style: no
/// column padding, no didactic footer). Pure over the DTO so it can be
/// unit-tested without spawning a child.
fn format_trace_summary(t: &SandboxTrace) -> String {
    // Platforms without a capture backend (macOS today) return an empty
    // CaptureKind::None trace — say so rather than print a fake "clean".
    if t.capture_kind == CaptureKind::None {
        return "jkr sandbox: behavioral trace not captured on this platform\n".to_string();
    }
    let mut s = String::new();
    let n = t.verdict.flags.len();
    s.push_str(&format!(
        "jkr sandbox: verdict={} ({n} flag{})\n",
        level_str(&t.verdict.level),
        if n == 1 { "" } else { "s" }
    ));
    for f in &t.verdict.flags {
        s.push_str(&format!(
            "  [{}] {}: {}\n",
            level_str(&f.severity),
            f.kind,
            f.detail
        ));
    }
    s.push_str(&format!(
        "files={} net={} exec={} denied={}{}\n",
        t.summary.files_total,
        t.summary.net_total,
        t.summary.exec_total,
        t.summary.denied_total,
        if t.truncated { " (truncated)" } else { "" }
    ));
    s
}

/// Fire-and-forget POST to jkr-server's `/api/v1/sandbox/ingest` so
/// the local run shows up in the dashboard's sandbox panel. Silent
/// on every failure mode (env unset, DNS, non-2xx, JSON encode) —
/// the CLI's job is not to break the user's local cmd flow. The 2s
/// timeout keeps tail latency bounded if the server is unreachable.
fn emit_ingest(command: &str, exit: i32, truncated: bool, duration_ms: u64) {
    // Preferred: credentials minted via `jkr login` and stored in
    // the OS keychain. Fallback: legacy env vars for headless setups
    // (CI, server boxes) where running `jkr login` interactively
    // doesn't make sense.
    let (base, token) = match crate::cmds::login::stored_credentials() {
        Some((u, t)) => (u, t),
        None => {
            let base = std::env::var("JKR_INGEST_URL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.trim().trim_end_matches('/').to_string());
            let token = std::env::var("JKR_INGEST_TOKEN")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.trim().to_string());
            match (base, token) {
                (Some(b), Some(t)) => (b, t),
                _ => return,
            }
        }
    };
    let url = format!("{base}/api/v1/sandbox/ingest");
    let body = serde_json::json!({
        "command": command,
        "exit": exit,
        "truncated": truncated,
        "duration_ms": duration_ms,
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(2)))
        .build()
        .into();
    let _ = agent
        .post(&url)
        .header("authorization", &format!("Bearer {token}"))
        .header("content-type", "application/json")
        .send_json(&body);
}

/// `jkr sandbox claude` — launch Claude Code (or any agent CLI) inside
/// the same Landlock/sandbox-exec jail `sandbox run` uses, with
/// defaults shaped for agent workflows:
///   * Read access to system libraries, the user's `~/.claude` config,
///     and `/tmp` — enough for the agent to boot, but nothing that
///     would let it exfiltrate `~/.ssh` or `/etc/shadow`.
///   * Write access to the current working directory + `/tmp` only.
///     The agent's Bash tool can edit the project but cannot reach
///     into the user's home outside the explicit allowlist.
///   * Forwards the env vars Claude Code needs to function
///     (`ANTHROPIC_*`, `CLAUDE_*`, terminal/locale plumbing) without
///     leaking the rest of the parent shell's environment.
///
/// The flags here add to those defaults — call with `--no-defaults` to
/// opt out entirely and grant individual paths/env explicitly via
/// `--read` / `--write` / `--env`.
#[allow(clippy::too_many_arguments)]
/// Default gateway the `claude` preset routes API calls through, so
/// traffic hits the proxy's redaction + capture path instead of going
/// straight to the provider.
const DEFAULT_GATEWAY_URL: &str = "https://tkr.prysm.sh";

/// Decide whether to inject `ANTHROPIC_BASE_URL`. Returns the URL to
/// set when the var is absent, or `None` when the operator already set
/// it (respect override — including a deliberate empty value). When the
/// user has logged in (`jkr login`), that server's URL takes precedence
/// over the compiled-in default so traffic lands on the same gateway
/// the captures are scoped against.
fn gateway_base_url_override(
    current: Option<&std::ffi::OsStr>,
    logged_in: Option<&str>,
) -> Option<String> {
    match current {
        Some(_) => None,
        None => Some(logged_in.unwrap_or(DEFAULT_GATEWAY_URL).to_string()),
    }
}

pub fn claude(
    extra_read: Vec<PathBuf>,
    extra_write: Vec<PathBuf>,
    extra_env: Vec<String>,
    no_defaults: bool,
    bin: String,
    agent_args: Vec<String>,
) -> Result<()> {
    // Compose the effective read/write/env lists. Defaults match what
    // `claude` actually needs at boot: Node + native deps under /usr
    // (Linux) or /opt + /Library (macOS), DNS/TLS config under /etc,
    // a writable scratch in /tmp, and the agent's own settings dir.
    let mut read: Vec<PathBuf> = Vec::new();
    let mut write: Vec<PathBuf> = Vec::new();
    let mut env: Vec<String> = Vec::new();

    if !no_defaults {
        let cwd = std::env::current_dir().context("read cwd")?;
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));

        // Read-only: everything the agent + its subprocesses need to
        // *exist on disk*. Order doesn't matter; duplicates are fine.
        let read_defaults = [
            cwd.clone(),
            home.join(".claude"),
            // Claude Code's primary config/state + auth lives in this single
            // file directly under $HOME (not inside ~/.claude). Without read
            // access the agent stalls at startup loading its config; without
            // write it can't persist updates.
            home.join(".claude.json"),
            home.join(".cache").join("claude-cli-nodejs"),
            home.join(".npm"),
            home.join(".nvm"),
            PathBuf::from("/usr"),
            PathBuf::from("/bin"),
            PathBuf::from("/lib"),
            PathBuf::from("/lib64"),
            PathBuf::from("/etc"),
            PathBuf::from("/tmp"),
            // macOS paths — harmless if absent on Linux.
            PathBuf::from("/opt"),
            PathBuf::from("/System"),
            PathBuf::from("/Library"),
            PathBuf::from("/private"),
            // Linux runtime info Claude Code touches for terminal /
            // node-pty plumbing.
            PathBuf::from("/proc"),
        ];
        for p in read_defaults {
            if p.exists() {
                read.push(p);
            }
        }

        // Write: only what the agent legitimately needs to modify.
        // The cwd is the workspace; ~/.claude needs write so caches +
        // tool databases update; /tmp for transient scratch.
        let write_defaults = [
            cwd,
            home.join(".claude"),
            home.join(".claude.json"),
            home.join(".cache").join("claude-cli-nodejs"),
            PathBuf::from("/tmp"),
        ];
        for p in write_defaults {
            if p.exists() || p.starts_with("/tmp") {
                write.push(p);
            }
        }

        // Route the agent's API calls through the jkr gateway so they
        // hit the proxy's redaction + capture path instead of going
        // straight to api.anthropic.com. Respect an explicit override:
        // if the operator already exported ANTHROPIC_BASE_URL we leave
        // it untouched. Setting it here (rather than pushing a
        // name=value) means the ANTHROPIC_ prefix pass below forwards
        // it like any other inherited var.
        let creds = crate::cmds::login::stored_credentials();
        let logged_in_url = creds.as_ref().map(|(u, _)| u.as_str());
        if let Some(url) =
            gateway_base_url_override(std::env::var_os("ANTHROPIC_BASE_URL").as_deref(), logged_in_url)
        {
            std::env::set_var("ANTHROPIC_BASE_URL", url);
        }

        // Attach the user's CLI token as a custom header so the gateway
        // can attribute captured calls to them (per-user scoping). Claude
        // Code forwards ANTHROPIC_CUSTOM_HEADERS onto every request; the
        // gateway reads `x-jkr-token` and never relays it upstream. Skip
        // if the user already set their own custom headers, or isn't
        // logged in.
        if std::env::var_os("ANTHROPIC_CUSTOM_HEADERS").is_none() {
            if let Some((_, token)) = creds.as_ref() {
                std::env::set_var(
                    "ANTHROPIC_CUSTOM_HEADERS",
                    format!("x-jkr-token: {token}"),
                );
            }
        }

        // Env vars: forward exact-match names that are essential, plus
        // a prefix pass over the parent env for anthropic/claude/openai
        // / node / npm / locale.  This is the closest we get to a
        // "glob" in the existing sandbox API — we enumerate matches
        // here and pass each by exact name.
        let exact = [
            "PATH", "HOME", "USER", "LOGNAME", "TERM", "EDITOR", "VISUAL",
            "SHELL", "PWD", "LANG", "TZ", "NO_COLOR", "FORCE_COLOR",
            "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY",
        ];
        for name in exact {
            env.push(name.to_string());
        }
        for (name, _) in std::env::vars() {
            let n = name.as_str();
            let matches = n.starts_with("ANTHROPIC_")
                || n.starts_with("CLAUDE_")
                || n.starts_with("OPENAI_")
                || n.starts_with("LC_")
                || n.starts_with("NODE_")
                || n.starts_with("NPM_")
                || n.starts_with("npm_");
            if matches {
                env.push(name);
            }
        }
    }

    read.extend(extra_read);
    write.extend(extra_write);
    env.extend(extra_env);

    eprintln!(
        "jkr sandbox claude: launching `{bin}` in jail \
         ({} read paths, {} write paths, {} env vars forwarded)",
        read.len(),
        write.len(),
        env.len()
    );

    let mut argv = vec![bin];
    argv.extend(agent_args);

    // Reuse the same `run()` plumbing. Network: leave open — Claude
    // Code needs Anthropic + MCP. Operators who want network locked
    // can drop to `sandbox run` directly with `--no-network` or
    // `--allow-connect 443`.
    // Claude path supplies its own curated read defaults — don't
    // layer system paths on top.
    run(
        false, read, write, env, None, None, None, None, false, vec![], vec![], false, false, true,
        argv,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use jkr_sandbox::capture::trace::{Flag, TraceSummary, Verdict};

    #[test]
    fn gateway_injected_when_base_url_absent() {
        assert_eq!(
            gateway_base_url_override(None, None).as_deref(),
            Some(DEFAULT_GATEWAY_URL),
            "absent ANTHROPIC_BASE_URL should default to the gateway"
        );
    }

    #[test]
    fn gateway_prefers_logged_in_url() {
        assert_eq!(
            gateway_base_url_override(None, Some("https://my.gw")).as_deref(),
            Some("https://my.gw"),
            "the logged-in server URL should win over the compiled default"
        );
    }

    #[test]
    fn gateway_respects_existing_override() {
        let custom = std::ffi::OsString::from("https://proxy.example");
        assert_eq!(
            gateway_base_url_override(Some(&custom), Some("https://my.gw")),
            None,
            "an operator-set URL must not be clobbered, even when logged in"
        );
        // A deliberate empty value is still "set" — leave it alone.
        let empty = std::ffi::OsString::from("");
        assert_eq!(gateway_base_url_override(Some(&empty), None), None);
    }

    fn trace_with(level: VerdictLevel, flags: Vec<Flag>, summary: TraceSummary) -> SandboxTrace {
        let mut t = SandboxTrace::none();
        t.capture_kind = CaptureKind::Full;
        t.verdict = Verdict { level, flags };
        t.summary = summary;
        t
    }

    #[test]
    fn summary_reports_none_capture() {
        let s = format_trace_summary(&SandboxTrace::none());
        assert!(s.contains("not captured on this platform"));
    }

    #[test]
    fn summary_clean_has_zero_flags() {
        let s = format_trace_summary(&trace_with(
            VerdictLevel::Clean,
            vec![],
            TraceSummary::default(),
        ));
        assert!(s.contains("verdict=clean (0 flags)"));
    }

    #[test]
    fn summary_lists_flags_and_counts() {
        let s = format_trace_summary(&trace_with(
            VerdictLevel::Suspicious,
            vec![Flag {
                kind: "sensitive_path".into(),
                severity: VerdictLevel::Suspicious,
                detail: "read /home/u/.ssh/id_rsa".into(),
            }],
            TraceSummary {
                files_total: 12,
                net_total: 2,
                exec_total: 1,
                denied_total: 4,
            },
        ));
        assert!(s.contains("verdict=suspicious (1 flag)"));
        assert!(s.contains("[suspicious] sensitive_path: read /home/u/.ssh/id_rsa"));
        assert!(s.contains("files=12 net=2 exec=1 denied=4"));
    }
}
