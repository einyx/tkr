//! Native **`go test`** — elide verbose `=== RUN` / `--- PASS` pairs (same idea as `cargo test`).
//! Env: **`TKR_NATIVE_GO_TEST=0`** disables.
//!
//! Skips shrinking when **`-json`**, **`-bench`**, or **`-fuzz`** is present (different output shape).

use super::NativeOutcome;
use crate::runner::stream_command;
use crate::stream::PipelineResult;
use anyhow::Result;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

pub fn env_disabled() -> bool {
    matches!(
        std::env::var("TKR_NATIVE_GO_TEST").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

static GO_VERBOSE_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^=== RUN\s+").expect("go === RUN"));

static GO_VERBOSE_PASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^--- PASS:").expect("go --- PASS"));

fn args_before_dd(args: &[String]) -> &[String] {
    let n = args.iter().position(|a| a == "--").unwrap_or(args.len());
    &args[..n]
}

/// Skip `-C dir`, `-modfile f`, `-overlay f` when looking for the `go` subcommand.
fn skip_go_leading_globals(args: &[String], mut i: usize) -> usize {
    while i < args.len() {
        match args[i].as_str() {
            "-C" | "-modfile" | "-overlay" => {
                i += 2;
            }
            a if a.starts_with("-C=")
                || a.starts_with("-modfile=")
                || a.starts_with("-overlay=") =>
            {
                i += 1;
            }
            _ => break,
        }
    }
    i
}

/// True when this is `go test …` (not `go help test`, `go run`, …).
fn subcommand_is_test(args: &[String]) -> bool {
    let a = args_before_dd(args);
    let i = skip_go_leading_globals(a, 0);
    i < a.len() && a[i] == "test"
}

fn uses_unsupported_output_mode(args: &[String]) -> bool {
    for x in args_before_dd(args) {
        let s = x.as_str();
        if s == "-json"
            || s == "-fuzz"
            || s.starts_with("-fuzz=")
            || s == "-bench"
            || s.starts_with("-bench=")
        {
            return true;
        }
    }
    false
}

fn should_elide_verbose_pass_noise(line: &str) -> bool {
    GO_VERBOSE_RUN.is_match(line) || GO_VERBOSE_PASS.is_match(line)
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<NativeOutcome>> {
    if Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|b| {
            let b = b.strip_suffix(".exe").unwrap_or(b);
            b.eq_ignore_ascii_case("go")
        })
        != Some(true)
    {
        return Ok(None);
    }
    if !subcommand_is_test(args) {
        return Ok(None);
    }
    if uses_unsupported_output_mode(args) {
        return Ok(None);
    }

    let str_args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut stream = stream_command(cmd, &str_args)?;
    let mut chars_in: u64 = 0;
    let mut emitted = String::new();
    let mut bytes_out: u64 = 0;
    let mut elided: u64 = 0;

    loop {
        let raw = match stream.next() {
            None => break,
            Some(Err(e)) => {
                eprintln!("tkr: native go test: {e}");
                continue;
            }
            Some(Ok(line)) => line,
        };
        chars_in += raw.len() as u64 + 1;

        if should_elide_verbose_pass_noise(&raw) {
            elided += 1;
            continue;
        }

        if elided > 0 {
            let msg = format!(
                "… ({} verbose pass/run lines elided — set `TKR_NATIVE_GO_TEST=0` for full log)\n",
                elided
            );
            bytes_out += msg.len() as u64;
            emitted.push_str(&msg);
            elided = 0;
        }

        bytes_out += raw.len() as u64 + 1;
        emitted.push_str(&raw);
        emitted.push('\n');
    }

    if elided > 0 {
        let msg = format!("… ({} verbose pass/run lines elided)\n", elided);
        bytes_out += msg.len() as u64;
        emitted.push_str(&msg);
    }

    let code = stream.wait_child()?;

    print!("{}", emitted);
    if !emitted.ends_with('\n') && !emitted.is_empty() {
        println!();
        bytes_out += 1;
    }

    let chars_suppressed = chars_in.saturating_sub(bytes_out);

    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![emitted],
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
    fn detects_go_test() {
        assert!(subcommand_is_test(&["test".into(), "./...".into()]));
        assert!(subcommand_is_test(&["-C".into(), "/tmp".into(), "test".into()]));
    }

    #[test]
    fn not_go_help_test() {
        assert!(!subcommand_is_test(&["help".into(), "test".into()]));
    }

    #[test]
    fn detects_json_bench_skip() {
        assert!(uses_unsupported_output_mode(&["test".into(), "-json".into()]));
        assert!(uses_unsupported_output_mode(&["test".into(), "-bench=.".into()]));
        assert!(!uses_unsupported_output_mode(&["test".into(), "-v".into(), "./...".into()]));
    }

    #[test]
    fn elide_patterns() {
        assert!(should_elide_verbose_pass_noise("=== RUN   TestFoo"));
        assert!(should_elide_verbose_pass_noise("--- PASS: TestFoo (0.00s)"));
        assert!(!should_elide_verbose_pass_noise("--- FAIL: TestFoo"));
    }
}
