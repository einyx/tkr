//! Native `ls`: stream output with line/byte caps (RTK-style summary for huge dirs).
//! Env: `JKR_NATIVE_LS=0` disables.

use super::NativeOutcome;
use crate::stream::PipelineResult;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

pub fn env_disabled() -> bool {
    matches!(
        std::env::var("JKR_NATIVE_LS").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

fn max_lines() -> usize {
    std::env::var("JKR_NATIVE_LS_MAX_LINES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(400)
}

fn max_line_bytes() -> usize {
    std::env::var("JKR_NATIVE_LS_MAX_LINE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(512)
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<NativeOutcome>> {
    let base = Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd);
    if base != "ls" {
        return Ok(None);
    }

    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning `{cmd}` (native ls)"))?;

    let stderr = child.stderr.take().expect("piped");
    let stderr_handle = std::thread::spawn(move || -> u64 {
        let mut n = 0u64;
        for line in BufReader::new(stderr).lines() {
            let Ok(l) = line else { break };
            let _ = writeln!(std::io::stderr().lock(), "{l}");
            n += l.len() as u64 + 1;
        }
        n
    });

    let stdout = child.stdout.take().expect("piped");
    let reader = BufReader::new(stdout);

    let ml = max_lines();
    let mlb = max_line_bytes();
    let mut out = String::new();
    let mut stdout_bytes: u64 = 0;
    let mut emitted = 0usize;
    let mut truncated_tail = false;

    for line in reader.lines() {
        let line = line?;
        stdout_bytes += line.len() as u64 + 1;

        if emitted >= ml {
            truncated_tail = true;
            continue;
        }
        out.push_str(&truncate_bytes(&line, mlb));
        out.push('\n');
        emitted += 1;
    }

    if truncated_tail {
        out.push_str(&format!(
            "… ({} lines max — raise JKR_NATIVE_LS_MAX_LINES)\n",
            ml
        ));
    }

    let status = child.wait().context("wait ls")?;
    let stderr_bytes = stderr_handle.join().unwrap_or(0);
    let code = status.code().unwrap_or(-1);
    let chars_in = stdout_bytes + stderr_bytes;
    let bytes_out = out.len() as u64;

    print!("{}", out);
    if !out.ends_with('\n') && !out.is_empty() {
        println!();
    }

    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![out],
            chars_in,
            chars_suppressed: chars_in.saturating_sub(bytes_out),
        },
        exit_code: code,
    }))
}

fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut t = String::new();
    for c in s.chars() {
        let w = c.len_utf8();
        if t.len() + w + 8 > max {
            break;
        }
        t.push(c);
    }
    t.push_str(" …");
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_bytes_smoke() {
        let s = "x".repeat(1000);
        let t = truncate_bytes(&s, 40);
        assert!(t.len() < s.len());
    }
}
