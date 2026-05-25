//! Native `cat` for simple positional file arguments (no flags). stdin (`cat`
//! alone) uses the normal TOML pipeline. Env: `JKR_NATIVE_READ`,
//! `JKR_NATIVE_READ_MAX_LINES`, `JKR_NATIVE_READ_MAX_LINE`.

use super::NativeOutcome;
use crate::stream::PipelineResult;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn env_disabled() -> bool {
    matches!(
        std::env::var("JKR_NATIVE_READ").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

fn max_lines() -> usize {
    std::env::var("JKR_NATIVE_READ_MAX_LINES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(400)
}

fn max_line_bytes() -> usize {
    std::env::var("JKR_NATIVE_READ_MAX_LINE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(800)
}

/// Returns `None` so the proxy falls back to spawning `cat` through the filter pipeline.
pub fn run(cmd: &str, args: &[String]) -> Result<Option<NativeOutcome>> {
    let base = Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd);
    if base != "cat" || args.is_empty() {
        return Ok(None);
    }
    if args.iter().any(|a| a.starts_with('-')) {
        return Ok(None);
    }

    let mut paths: Vec<PathBuf> = Vec::with_capacity(args.len());
    for a in args {
        if a.contains("..") {
            return Ok(None);
        }
        let p = PathBuf::from(a);
        if !p.is_file() {
            return Ok(None);
        }
        paths.push(p);
    }

    let ml = max_lines();
    let mlb = max_line_bytes();
    let mut combined = String::new();
    let mut chars_in: u64 = 0;

    for p in &paths {
        let raw =
            std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
        chars_in += raw.len() as u64;
        let body = shrink_file(&raw, ml, mlb);
        if paths.len() > 1 {
            combined.push_str(&format!("── {} ──\n", p.display()));
        }
        combined.push_str(&body);
        if !body.ends_with('\n') {
            combined.push('\n');
        }
    }

    let bytes_out = combined.len() as u64;
    print!("{}", combined);
    let suppressed = chars_in.saturating_sub(bytes_out);

    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![combined],
            chars_in,
            chars_suppressed: suppressed,
        },
        exit_code: 0,
    }))
}

fn shrink_file(raw: &str, max_lines: usize, max_line_bytes: usize) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = raw.lines().take(max_lines).collect();
    let truncated_file = raw.lines().count() > max_lines;
    for line in lines {
        out.push_str(&truncate_line_bytes(line, max_line_bytes));
        out.push('\n');
    }
    if truncated_file {
        out.push_str(&format!(
            "... ({max_lines} lines max — raise JKR_NATIVE_READ_MAX_LINES)\n"
        ));
    }
    out
}

fn truncate_line_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut t = String::new();
    for c in s.chars() {
        let w = c.len_utf8();
        if t.len() + w + 12 > max {
            break;
        }
        t.push(c);
    }
    t.push_str(" … (+truncated)");
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_long_line() {
        let s = "a".repeat(2000);
        let t = truncate_line_bytes(&s, 80);
        assert!(t.len() < 2000);
        assert!(t.contains("truncated"));
    }
}
