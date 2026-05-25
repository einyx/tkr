//! Native `git` optimizations: `status -> -sb`, `diff` unified-diff condense,
//! and RTK-style one-line success summaries for `add` / `commit` / `push` / `pull`.
//!
//! Env: `JKR_NATIVE_GIT=0` disables all native git paths.
//! `JKR_NATIVE_GIT_DIFF=0` skips diff condense only.
//! `JKR_NATIVE_GIT_COMPACT=0` keeps **`add` / `commit` / `push` / `pull`** on the
//! streaming + `filters/git.toml` pipeline (full porcelain).

use super::NativeOutcome;
use crate::stream::PipelineResult;
use anyhow::{Context, Result};
use regex::Regex;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

const MAX_DIFF_BYTES: usize = 8 * 1024 * 1024;

static COMMIT_SUMMARY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\[([^\]\s]+)\s+([0-9a-f]{7,40})\]\s*(.*)$").expect("commit summary regex")
});

static PUSH_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s+\S+\s+(\S+)\s+->\s+(\S+)\s*$").expect("push ref regex")
});

static PUSH_NEW_BRANCH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\*\s*\[new branch\]\s+\S+\s+->\s+(\S+)\s*$").expect("push new branch regex")
});

static PULL_FILES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(\d+) files? changed(?:,\s*(\d+) insertions?\(\+\))?(?:,\s*(\d+) deletions?\(-\))?\s*$",
    )
    .expect("pull files-changed regex")
});

pub fn env_disabled() -> bool {
    matches!(
        std::env::var("JKR_NATIVE_GIT").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

fn diff_native_disabled() -> bool {
    matches!(
        std::env::var("JKR_NATIVE_GIT_DIFF").ok().as_deref(),
        Some("0") | Some("false") | Some("off") | Some("passthrough")
    )
}

fn compact_disabled() -> bool {
    matches!(
        std::env::var("JKR_NATIVE_GIT_COMPACT").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<NativeOutcome>> {
    let base = Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd);
    if base != "git" {
        return Ok(None);
    }

    if let Some(di) = args.iter().position(|a| a == "diff") {
        let r = run_diff(cmd, args, di)?;
        if r.is_some() {
            return Ok(r);
        }
    }

    if let Some(si) = args.iter().position(|a| a == "status") {
        return run_status(cmd, args, si);
    }

    if !compact_disabled() {
        if let Some(ci) = git_command_start(args) {
            match args.get(ci).map(|s| s.as_str()) {
                Some("add") => {
                    if let Some(o) = run_add_compact(cmd, args, ci)? {
                        return Ok(Some(o));
                    }
                }
                Some("commit") => {
                    if let Some(o) = run_commit_compact(cmd, args, ci)? {
                        return Ok(Some(o));
                    }
                }
                Some("push") => {
                    if let Some(o) = run_push_compact(cmd, args, ci)? {
                        return Ok(Some(o));
                    }
                }
                Some("pull") => {
                    if let Some(o) = run_pull_compact(cmd, args, ci)? {
                        return Ok(Some(o));
                    }
                }
                _ => {}
            }
        }
    }

    Ok(None)
}

/// Index of the git subcommand (`add`, `push`, …), skipping leading global options.
fn git_command_start(args: &[String]) -> Option<usize> {
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-C" | "-c" if i + 1 < args.len() => i += 2,
            "--exec-path" | "--git-dir" | "--work-tree" | "--namespace"
                if i + 1 < args.len() && !args[i + 1].starts_with('-') =>
            {
                i += 2;
            }
            _ if a.starts_with("--exec-path=")
                || a.starts_with("--git-dir=")
                || a.starts_with("--work-tree=")
                || a.starts_with("--namespace=") =>
            {
                i += 1;
            }
            _ if a.starts_with('-') => i += 1,
            _ => return Some(i),
        }
    }
    None
}

fn dump_git_output(out: &std::process::Output) {
    let _ = std::io::stdout().write_all(&out.stdout);
    let _ = std::io::stderr().write_all(&out.stderr);
}

fn lossy_combined(out: &std::process::Output) -> String {
    let mut s = String::new();
    s.push_str(&String::from_utf8_lossy(&out.stdout));
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    s
}

fn simplify_git_ref(r: &str) -> String {
    let tail = r
        .strip_prefix("refs/heads/")
        .or_else(|| r.strip_prefix("refs/remotes/"))
        .unwrap_or(r);
    tail.rsplit('/').next().unwrap_or(tail).to_string()
}

fn parse_commit_success_summary(combined: &str) -> String {
    COMMIT_SUMMARY_RE
        .captures(combined)
        .map(|c| {
            let sha = c.get(2).map(|m| m.as_str()).unwrap_or("");
            let mut subj = c.get(3).map(|m| m.as_str()).unwrap_or("").trim().to_string();
            if subj.chars().count() > 72 {
                subj = subj.chars().take(69).collect::<String>() + "…";
            }
            if subj.is_empty() {
                format!("ok · {sha}\n")
            } else {
                format!("ok · {sha} · {subj}\n")
            }
        })
        .unwrap_or_else(|| "ok · commit\n".to_string())
}

fn summarize_push_success(combined: &str) -> String {
    if combined.contains("Everything up-to-date") || combined.contains("Everything up to date") {
        return "ok · up to date\n".to_string();
    }
    for line in combined.lines().rev() {
        if let Some(caps) = PUSH_NEW_BRANCH_RE.captures(line) {
            let b = caps.get(1).map(|m| m.as_str()).unwrap_or("?");
            return format!("ok · {}\n", simplify_git_ref(b));
        }
        if let Some(caps) = PUSH_REF_RE.captures(line) {
            let remote_side = caps.get(2).map(|m| m.as_str()).unwrap_or("?");
            return format!("ok · {}\n", simplify_git_ref(remote_side));
        }
    }
    "ok · push\n".to_string()
}

fn summarize_pull_success(combined: &str) -> String {
    if combined.contains("Already up to date.") {
        return "ok · up to date\n".to_string();
    }
    for line in combined.lines().rev() {
        if let Some(caps) = PULL_FILES_RE.captures(line) {
            let files = caps.get(1).map(|m| m.as_str()).unwrap_or("0");
            let ins = caps
                .get(2)
                .map(|m| m.as_str().parse::<u64>().unwrap_or(0))
                .unwrap_or(0);
            let del = caps
                .get(3)
                .map(|m| m.as_str().parse::<u64>().unwrap_or(0))
                .unwrap_or(0);
            return format!("ok · {files}f +{ins} -{del}\n");
        }
    }
    "ok · pull\n".to_string()
}

fn add_wants_passthrough(tail: &[String]) -> bool {
    tail.iter().any(|a| {
        matches!(
            a.as_str(),
            "-i" | "-p" | "--patch" | "--interactive" | "-n" | "--dry-run"
        ) || a.starts_with("--dry-run")
    })
}

fn commit_wants_passthrough(tail: &[String]) -> bool {
    tail.iter().any(|a| matches!(a.as_str(), "-v" | "--verbose" | "--dry-run"))
}

fn commit_can_compact_noninteractive(tail: &[String]) -> bool {
    let mut i = 0usize;
    while i < tail.len() {
        match tail[i].as_str() {
            "-m" | "-F" | "-t" | "-C" | "-c" | "--file" => return true,
            "--no-edit" | "--allow-empty-message" => return true,
            "--reuse-message" | "--reedit-message" => return true,
            s if s.starts_with("--reuse-message=")
                || s.starts_with("--reedit-message=")
                || s.starts_with("--file=") =>
            {
                return true;
            }
            s if s.starts_with("-m") && s.len() > 2 && !s.starts_with("--") => return true,
            _ => {}
        }
        i += 1;
    }
    false
}

fn push_pull_wants_passthrough(tail: &[String]) -> bool {
    tail.iter().any(|a| {
        matches!(a.as_str(), "-n" | "--dry-run" | "--progress") || a.starts_with("--dry-run")
    })
}

fn run_add_compact(cmd: &str, args: &[String], ci: usize) -> Result<Option<NativeOutcome>> {
    let tail = &args[ci + 1..];
    if add_wants_passthrough(tail) {
        return Ok(None);
    }

    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;

    let chars_in = out.stdout.len() as u64 + out.stderr.len() as u64;
    let code = out.status.code().unwrap_or(-1);

    if code != 0 {
        dump_git_output(&out);
        let combined = lossy_combined(&out);
        return Ok(Some(NativeOutcome {
            pipeline: PipelineResult {
                emitted: vec![combined],
                chars_in,
                chars_suppressed: 0,
            },
            exit_code: code,
        }));
    }

    let line = "ok\n".to_string();
    print!("{}", line);
    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![line.clone()],
            chars_in,
            chars_suppressed: chars_in.saturating_sub(line.len() as u64),
        },
        exit_code: 0,
    }))
}

fn run_commit_compact(cmd: &str, args: &[String], ci: usize) -> Result<Option<NativeOutcome>> {
    let tail = &args[ci + 1..];
    if commit_wants_passthrough(tail) || !commit_can_compact_noninteractive(tail) {
        return Ok(None);
    }

    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;

    let chars_in = out.stdout.len() as u64 + out.stderr.len() as u64;
    let code = out.status.code().unwrap_or(-1);
    let combined = lossy_combined(&out);

    if code != 0 {
        dump_git_output(&out);
        return Ok(Some(NativeOutcome {
            pipeline: PipelineResult {
                emitted: vec![combined],
                chars_in,
                chars_suppressed: 0,
            },
            exit_code: code,
        }));
    }

    let line = parse_commit_success_summary(&combined);
    print!("{}", line);
    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![line.clone()],
            chars_in,
            chars_suppressed: chars_in.saturating_sub(line.len() as u64),
        },
        exit_code: 0,
    }))
}

fn run_push_compact(cmd: &str, args: &[String], ci: usize) -> Result<Option<NativeOutcome>> {
    let tail = &args[ci + 1..];
    if push_pull_wants_passthrough(tail) {
        return Ok(None);
    }

    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;

    let chars_in = out.stdout.len() as u64 + out.stderr.len() as u64;
    let code = out.status.code().unwrap_or(-1);
    let combined = lossy_combined(&out);

    if code != 0 {
        dump_git_output(&out);
        return Ok(Some(NativeOutcome {
            pipeline: PipelineResult {
                emitted: vec![combined],
                chars_in,
                chars_suppressed: 0,
            },
            exit_code: code,
        }));
    }

    let line = summarize_push_success(&combined);
    print!("{}", line);
    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![line.clone()],
            chars_in,
            chars_suppressed: chars_in.saturating_sub(line.len() as u64),
        },
        exit_code: 0,
    }))
}

fn run_pull_compact(cmd: &str, args: &[String], ci: usize) -> Result<Option<NativeOutcome>> {
    let tail = &args[ci + 1..];
    if push_pull_wants_passthrough(tail) {
        return Ok(None);
    }

    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;

    let chars_in = out.stdout.len() as u64 + out.stderr.len() as u64;
    let code = out.status.code().unwrap_or(-1);
    let combined = lossy_combined(&out);

    if code != 0 {
        dump_git_output(&out);
        return Ok(Some(NativeOutcome {
            pipeline: PipelineResult {
                emitted: vec![combined],
                chars_in,
                chars_suppressed: 0,
            },
            exit_code: code,
        }));
    }

    let line = summarize_pull_success(&combined);
    print!("{}", line);
    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![line.clone()],
            chars_in,
            chars_suppressed: chars_in.saturating_sub(line.len() as u64),
        },
        exit_code: 0,
    }))
}

fn run_diff(cmd: &str, args: &[String], di: usize) -> Result<Option<NativeOutcome>> {
    if diff_native_disabled() {
        return Ok(None);
    }

    let tail = &args[di + 1..];
    if tail.iter().any(|a| {
        matches!(
            a.as_str(),
            "--stat" | "--numstat" | "--shortstat" | "--name-only" | "--name-status" | "--check"
        )
    }) || tail.iter().any(|a| a.starts_with("--output"))
    {
        return Ok(None);
    }
    if tail.iter().any(|a| a.starts_with("--word-diff") || a == "-W") {
        return Ok(None);
    }

    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running `git {}`", args.join(" ")))?;

    if out.stdout.len() > MAX_DIFF_BYTES {
        return Ok(None);
    }

    let code = out.status.code().unwrap_or(-1);
    let chars_in = out.stdout.len() as u64 + out.stderr.len() as u64;

    if !out.stderr.is_empty() {
        let _ = std::io::stderr().write_all(&out.stderr);
    }

    let raw = String::from_utf8_lossy(&out.stdout);
    let condensed = condense_unified_diff(&raw);
    let stdout_saved = (out.stdout.len() as u64).saturating_sub(condensed.len() as u64);
    let chars_suppressed = stdout_saved;

    print!("{}", condensed);
    if !condensed.is_empty() && !condensed.ends_with('\n') {
        println!();
    }

    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![condensed],
            chars_in,
            chars_suppressed,
        },
        exit_code: code,
    }))
}

/// Match `git.toml` intent: drop index noise, shorten @@, collapse context runs.
fn condense_unified_diff(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(512 * 1024));
    let mut in_ctx = false;
    let mut ctx_elide = 0u32;

    for line in input.lines() {
        if line.starts_with("index ")
            || line.starts_with("similarity index ")
            || line.starts_with("dissimilarity index ")
            || line == r"\ No newline at end of file"
        {
            continue;
        }

        if line.starts_with("@@ ") {
            if in_ctx && ctx_elide > 0 {
                out.push_str(&format!(" … ({} context lines elided)\n", ctx_elide));
            }
            in_ctx = false;
            ctx_elide = 0;
            out.push_str(&truncate_hunk_header(line));
            out.push('\n');
            continue;
        }

        if is_unified_context_line(line) {
            if !in_ctx {
                out.push_str(line);
                out.push('\n');
                in_ctx = true;
            } else {
                ctx_elide += 1;
            }
            continue;
        }

        if in_ctx && ctx_elide > 0 {
            out.push_str(&format!(" … ({} context lines elided)\n", ctx_elide));
        }
        in_ctx = false;
        ctx_elide = 0;

        out.push_str(line);
        out.push('\n');
    }

    if in_ctx && ctx_elide > 0 {
        out.push_str(&format!(
            " … ({} context lines elided)\n",
            ctx_elide
        ));
    }

    out
}

#[inline]
fn is_unified_context_line(line: &str) -> bool {
    line.as_bytes().first() == Some(&b' ')
}

fn truncate_hunk_header(line: &str) -> String {
    if line.len() <= 160 {
        return line.to_string();
    }
    if let Some(idx) = line.find(" @@") {
        return line[..idx.saturating_add(3)].to_string();
    }
    let mut s: String = line.chars().take(160).collect();
    s.push_str(" …");
    s
}

fn run_status(cmd: &str, args: &[String], si: usize) -> Result<Option<NativeOutcome>> {
    if args
        .iter()
        .any(|a| a == "--porcelain" || a.starts_with("--porcelain="))
    {
        return Ok(None);
    }
    if args[si + 1..]
        .iter()
        .any(|a| a == "-v" || a == "--verbose" || a == "--show-stash" || a == "--long")
    {
        return Ok(None);
    }

    let tail = &args[si + 1..];
    let already_short = tail
        .iter()
        .any(|a| matches!(a.as_str(), "-s" | "--short" | "-sb"));

    let mut resolved = args.to_vec();
    if !already_short {
        resolved.insert(si + 1, "-sb".into());
    }

    let out = Command::new(cmd)
        .args(&resolved)
        .output()
        .with_context(|| format!("running `git {}`", resolved.join(" ")))?;

    let code = out.status.code().unwrap_or(-1);
    let chars_in = out.stdout.len() as u64 + out.stderr.len() as u64;

    if !out.stderr.is_empty() {
        let _ = std::io::stderr().write_all(&out.stderr);
    }

    let stdout_raw = String::from_utf8_lossy(&out.stdout);
    print!("{}", stdout_raw);
    if !stdout_raw.is_empty() && !stdout_raw.ends_with('\n') {
        println!();
    }

    let bytes_out = out.stdout.len() as u64;
    let chars_suppressed = chars_in.saturating_sub(bytes_out);

    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![stdout_raw.into_owned()],
            chars_in,
            chars_suppressed,
        },
        exit_code: code,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_command_start_skips_globals() {
        let args = vec![
            "-C".into(),
            "/repo".into(),
            "-c".into(),
            "user.name=x".into(),
            "push".into(),
            "origin".into(),
        ];
        assert_eq!(git_command_start(&args), Some(4));
        assert_eq!(git_command_start(&["status".into()]), Some(0));
    }

    #[test]
    fn parse_commit_success_summary_extracts_sha_and_subject() {
        let text = "[main abcdef1] fix typo\n 1 file changed, 2 insertions(+), 1 deletion(-)\n";
        assert_eq!(
            parse_commit_success_summary(text),
            "ok · abcdef1 · fix typo\n"
        );
    }

    #[test]
    fn summarize_push_branch_and_up_to_date() {
        assert_eq!(
            summarize_push_success("Everything up-to-date\n"),
            "ok · up to date\n"
        );
        let out = "To github.com:a/b.git\n   abc..def  main -> main\n";
        assert_eq!(summarize_push_success(out), "ok · main\n");
    }

    #[test]
    fn summarize_pull_stats_and_up_to_date() {
        assert_eq!(
            summarize_pull_success("Already up to date.\n"),
            "ok · up to date\n"
        );
        let out = "Fast-forward\n README.md | 5 ++---\n 3 files changed, 10 insertions(+), 2 deletions(-)\n";
        assert_eq!(summarize_pull_success(out), "ok · 3f +10 -2\n");
    }

    #[test]
    fn simplify_git_ref_strips_refs_heads() {
        assert_eq!(simplify_git_ref("refs/heads/feature/foo"), "foo");
    }

    #[test]
    fn condense_drops_index_and_collapses_context() {
        let raw = r#"diff --git a/x b/x
index 9f3a..e42 100644
--- a/x
+++ b/x
@@ -1,4 +1,4 @@
 oldctx1
 oldctx2
-old
+new
 tail
"#;
        let c = condense_unified_diff(raw);
        assert!(!c.contains("index "));
        assert!(c.contains("context lines elided"));
        assert!(c.contains("-old"));
        assert!(c.contains("+new"));
    }
}
