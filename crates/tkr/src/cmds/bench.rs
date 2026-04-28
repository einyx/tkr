//! `tkr bench <cmd>` — measure raw vs filtered output for a command, report
//! savings ratio. Useful for tuning filters and showing users the win on
//! their actual workloads.

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

pub fn run(parts: &[String]) -> Result<()> {
    if parts.is_empty() {
        anyhow::bail!("usage: tkr bench <command> [args...]");
    }
    let (cmd, args) = parts.split_first().unwrap();
    let arg_strs: Vec<&str> = args.iter().map(String::as_str).collect();

    println!("Benchmarking: {} {}", cmd, args.join(" "));

    // 1. Raw run — capture combined stdout+stderr.
    let raw = capture_combined(cmd, &arg_strs).context("running raw command")?;

    // 2. Filtered run — invoke ourselves so the proxy pipeline runs.
    let me = std::env::current_exe().context("locating current tkr binary")?;
    let mut full_args: Vec<String> = vec![cmd.clone()];
    full_args.extend(args.iter().cloned());
    let filt = capture_combined(
        me.to_string_lossy().as_ref(),
        &full_args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .context("running filtered command")?;

    let raw_chars = raw.len();
    let filt_chars = filt.len();
    let saved = raw_chars.saturating_sub(filt_chars);
    let ratio = if raw_chars > 0 {
        (saved as f64 / raw_chars as f64) * 100.0
    } else {
        0.0
    };

    println!();
    println!("  raw output     {:>10} chars  ({} tokens approx)", fmt(raw_chars), fmt(raw_chars / 4));
    println!("  filtered       {:>10} chars  ({} tokens approx)", fmt(filt_chars), fmt(filt_chars / 4));
    println!("  saved          {:>10} chars", fmt(saved));
    println!("  reduction      {:>9.1}%", ratio);
    println!();
    Ok(())
}

fn capture_combined(cmd: &str, args: &[&str]) -> Result<String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning `{cmd}`"))?;
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        s.read_to_string(&mut out).ok();
    }
    if let Some(mut s) = child.stderr.take() {
        let mut tmp = String::new();
        s.read_to_string(&mut tmp).ok();
        out.push_str(&tmp);
    }
    child.wait().ok();
    Ok(out)
}

fn fmt(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::new();
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

// Silence dead-import lint when this file is compiled standalone; Write isn't used here.
#[allow(dead_code)]
fn _unused_write(_w: &mut dyn Write) {}
