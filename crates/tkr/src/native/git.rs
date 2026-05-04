//! Native `git` optimizations: `status -> -sb`, `diff` unified-diff condense.
//! Env: `TKR_NATIVE_GIT=0` disables all. `TKR_NATIVE_GIT_DIFF=0` skips diff condense only.

use super::NativeOutcome;
use crate::stream::PipelineResult;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::Command;

const MAX_DIFF_BYTES: usize = 8 * 1024 * 1024;

pub fn env_disabled() -> bool {
    matches!(
        std::env::var("TKR_NATIVE_GIT").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

fn diff_native_disabled() -> bool {
    matches!(
        std::env::var("TKR_NATIVE_GIT_DIFF").ok().as_deref(),
        Some("0") | Some("false") | Some("off") | Some("passthrough")
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

    Ok(None)
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
