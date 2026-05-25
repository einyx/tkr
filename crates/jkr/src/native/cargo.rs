//! Native `cargo test` — collapse passing `test ... ok` spam; sample compile noise.
//! Env: `JKR_NATIVE_CARGO_TEST=0` disables. `JKR_NATIVE_CARGO_COMPILE_LINES` (default 8).

use super::NativeOutcome;
use crate::runner::stream_command;
use crate::stream::PipelineResult;
use anyhow::Result;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

pub fn env_disabled() -> bool {
    matches!(
        std::env::var("JKR_NATIVE_CARGO_TEST").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

fn max_compile_lines_shown() -> usize {
    std::env::var("JKR_NATIVE_CARGO_COMPILE_LINES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8)
}

static TEST_OK_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^test .+ \.\.\. ok( \(\d+\.\d+s\))?$").expect("test ok regex")
});

static COMPILE_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(Compiling|Checking|Downloading|Downloaded|Updating|Locking|Blocking|Fresh)\s")
        .expect("compile regex")
});

pub fn run(cmd: &str, args: &[String]) -> Result<Option<NativeOutcome>> {
    let base = Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd);
    if base != "cargo" {
        return Ok(None);
    }
    if find_test_subcommand(args).is_none() {
        return Ok(None);
    }

    let str_args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut stream = stream_command(cmd, &str_args)?;
    let mut chars_in: u64 = 0;
    let mut emitted = String::new();
    let mut bytes_out: u64 = 0;

    let mut ok_elided: u64 = 0;
    let mut compile_shown = 0usize;
    let compile_cap = max_compile_lines_shown();
    let mut compile_elided: u64 = 0;

    loop {
        let raw = match stream.next() {
            None => break,
            Some(Err(e)) => {
                eprintln!("jkr: native cargo test: {e}");
                continue;
            }
            Some(Ok(l)) => l,
        };
        chars_in += raw.len() as u64 + 1;

        if TEST_OK_LINE.is_match(&raw) {
            ok_elided += 1;
            continue;
        }
        if ok_elided > 0 {
            let msg = format!(
                "… ({} passing test lines elided — set `JKR_NATIVE_CARGO_TEST=0` for full log)\n",
                ok_elided
            );
            bytes_out += msg.len() as u64;
            emitted.push_str(&msg);
            ok_elided = 0;
        }

        if COMPILE_LINE.is_match(&raw) {
            if compile_shown < compile_cap {
                compile_shown += 1;
                bytes_out += raw.len() as u64 + 1;
                emitted.push_str(&raw);
                emitted.push('\n');
            } else {
                compile_elided += 1;
            }
            continue;
        }
        if compile_elided > 0 {
            let msg = format!("… ({} compile/check lines elided)\n", compile_elided);
            bytes_out += msg.len() as u64;
            emitted.push_str(&msg);
            compile_elided = 0;
        }

        bytes_out += raw.len() as u64 + 1;
        emitted.push_str(&raw);
        emitted.push('\n');
    }

    if ok_elided > 0 {
        let msg = format!("… ({} passing test lines elided)\n", ok_elided);
        bytes_out += msg.len() as u64;
        emitted.push_str(&msg);
    }
    if compile_elided > 0 {
        let msg = format!("… ({} compile/check lines elided)\n", compile_elided);
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

fn find_test_subcommand(args: &[String]) -> Option<usize> {
    let mut after_dd = false;
    for (i, a) in args.iter().enumerate() {
        if a == "--" {
            after_dd = true;
        }
        if !after_dd && a == "test" {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_test_after_globals() {
        let a = vec![
            "-C".into(),
            "/tmp".into(),
            "test".into(),
            "-q".into(),
        ];
        assert_eq!(find_test_subcommand(&a), Some(2));
    }

    #[test]
    fn subcommand_test_not_matched_after_double_dash() {
        let a = vec!["run".into(), "--".into(), "test".into()];
        assert!(find_test_subcommand(&a).is_none());
    }

    #[test]
    fn test_ok_line_matches() {
        assert!(TEST_OK_LINE.is_match("test foo::bar ... ok"));
        assert!(TEST_OK_LINE.is_match("test foo::bar ... ok (0.01s)"));
        assert!(!TEST_OK_LINE.is_match("test foo::bar ... FAILED"));
    }
}
