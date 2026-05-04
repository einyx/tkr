//! Native **`npm`** / **`pnpm`** / **`yarn`** / **`npx`** / **`bunx`** (and **`corepack …`**),
//! plus **`deno test`** / **`deno task …`** and **`bun test`** / **`bun run …`**,
//! or **standalone** **`jest`**, **`vitest`**, **`mocha`**, **`playwright test`**, **`cypress run`**:
//! elide noisy passing lines (vitest ✓, jest PASS, **`deno` `… ok`**, **`bun` `(pass)`**).
//! Env: `TKR_NATIVE_JS_TEST=0` disables.

use super::NativeOutcome;
use crate::runner::stream_command;
use crate::stream::PipelineResult;
use anyhow::Result;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

pub fn env_disabled() -> bool {
    matches!(
        std::env::var("TKR_NATIVE_JS_TEST").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

/// Vitest-style check / jest-style PASS lines (passing only).
static PASS_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*✓\s|^\s*✔\s|^PASS\s")
        .expect("js test pass regex")
});

/// Deno: `test scope … ok (1ms)`.
static DENO_TEST_OK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^test .+ \.\.\. ok(\s+\(\d+(?:\.\d+)?ms\))?\s*$").expect("deno test ok regex")
});

/// Bun default reporter: `(pass) description …`.
static BUN_PASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\(pass\)").expect("bun pass regex")
});

fn strip_exe_suffix(s: &str) -> &str {
    s.strip_suffix(".exe")
        .or_else(|| s.strip_suffix(".cmd"))
        .or_else(|| s.strip_suffix(".CMD"))
        .unwrap_or(s)
}

/// `(effective_tool, arg_tail)` — `corepack yarn test` → `("yarn", ["test"])`.
fn resolved_tool_and_args<'a>(cmd: &'a str, args: &'a [String]) -> (String, &'a [String]) {
    let base = Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd);
    let base = strip_exe_suffix(base);
    if base.eq_ignore_ascii_case("corepack") && !args.is_empty() {
        let sub = strip_exe_suffix(&args[0]);
        return (sub.to_ascii_lowercase(), &args[1..]);
    }
    (base.to_ascii_lowercase(), args)
}

fn npx_first_command(args: &[String]) -> Option<&str> {
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "-y"
            || a == "--yes"
            || a == "-yes"
            || a.starts_with("--package=")
            || a == "--call"
        {
            i += 1;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(a);
    }
    None
}

fn deno_test_like(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    if args[0] == "test" {
        return true;
    }
    if args.len() >= 2 && args[0] == "task" {
        let name = args[1].to_ascii_lowercase();
        return name == "test"
            || name.contains("test:")
            || name.contains("spec")
            || name.contains("vitest");
    }
    false
}

fn bun_test_like(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    if args[0] == "test" {
        return true;
    }
    if args.len() >= 2 && args[0] == "run" {
        let name = args[1].to_ascii_lowercase();
        return name == "test"
            || name.contains("test")
            || name.contains("vitest")
            || name.contains("jest");
    }
    false
}

fn npx_looks_like_test_runner(args: &[String]) -> bool {
    let Some(bin) = npx_first_command(args) else {
        return false;
    };
    let b = bin.to_ascii_lowercase();
    matches!(
        b.as_str(),
        "vitest" | "jest" | "playwright" | "cypress" | "mocha"
    ) || b.ends_with("/vitest")
        || b.ends_with("/jest")
        || b.ends_with("/bin/vitest")
        || b.ends_with("/bin/jest")
        || b == "@playwright/test"
        || b.starts_with("playwright")
}

/// True for globally installed runner binaries (`tkr jest`, `tkr vitest`, …).
fn standalone_test_runner(tool: &str, args: &[String]) -> Option<bool> {
    match tool {
        "jest" | "vitest" | "mocha" => {
            if let Some(a) = args.first() {
                if matches!(a.as_str(), "--help" | "-h" | "--version") {
                    return Some(false);
                }
                if tool == "jest" && a == "--init" {
                    return Some(false);
                }
            }
            Some(true)
        }
        "playwright" => Some(args.first().map(|s| s.as_str()) == Some("test")),
        "cypress" => Some(matches!(args.first().map(|s| s.as_str()), Some("run"))),
        _ => None,
    }
}

fn args_look_like_tests(tool: &str, args: &[String]) -> bool {
    if let Some(ok) = standalone_test_runner(tool, args) {
        return ok;
    }

    match tool {
        "deno" => return deno_test_like(args),
        "bun" => return bun_test_like(args),
        "npx" | "bunx" => return npx_looks_like_test_runner(args),
        _ => {}
    }

    if args.is_empty() {
        return false;
    }
    let a0 = args[0].as_str();

    match a0 {
        "test" | "t" => true,
        "run" if args.len() >= 2 => {
            let name = args[1].to_ascii_lowercase();
            name.contains("test")
                || name.contains("vitest")
                || name.contains("jest")
                || name.contains("e2e")
                || name.contains("spec")
        }
        "exec" if args.len() >= 2 => {
            let x = args[1].to_ascii_lowercase();
            x.contains("vitest")
                || x.contains("jest")
                || x.contains("playwright")
        }
        "vitest" | "jest" | "playwright" => true,
        _ => false,
    }
}

fn eligible(cmd: &str, args: &[String]) -> bool {
    let (tool, tail) = resolved_tool_and_args(cmd, args);
    matches!(
        tool.as_str(),
        "npm" | "pnpm" | "yarn" | "npx" | "bunx" | "deno" | "bun"
            | "jest" | "vitest" | "mocha" | "playwright" | "cypress"
    ) && args_look_like_tests(&tool, tail)
}

fn should_elide_pass_noise(line: &str) -> bool {
    if line.to_ascii_uppercase().contains("FAIL") {
        return false;
    }
    if line.contains('✖') || line.contains("× ") {
        return false;
    }
    PASS_LINE.is_match(line) || DENO_TEST_OK.is_match(line) || BUN_PASS.is_match(line)
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<NativeOutcome>> {
    if !eligible(cmd, args) {
        return Ok(None);
    }

    let str_args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut stream = stream_command(cmd, &str_args)?;
    let mut chars_in: u64 = 0;
    let mut emitted = String::new();
    let mut bytes_out: u64 = 0;
    let mut pass_elided: u64 = 0;

    loop {
        let raw = match stream.next() {
            None => break,
            Some(Err(e)) => {
                eprintln!("tkr: native js test: {e}");
                continue;
            }
            Some(Ok(line)) => line,
        };
        chars_in += raw.len() as u64 + 1;

        if should_elide_pass_noise(&raw) {
            pass_elided += 1;
            continue;
        }

        if pass_elided > 0 {
            let msg = format!(
                "… ({} passing-style lines elided — set `TKR_NATIVE_JS_TEST=0` for full log)\n",
                pass_elided
            );
            bytes_out += msg.len() as u64;
            emitted.push_str(&msg);
            pass_elided = 0;
        }

        bytes_out += raw.len() as u64 + 1;
        emitted.push_str(&raw);
        emitted.push('\n');
    }

    if pass_elided > 0 {
        let msg = format!("… ({} passing-style lines elided)\n", pass_elided);
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
    fn eligible_npm_test() {
        assert!(eligible("npm", &["test".into(), "-w".into(), "pkg".into()]));
    }

    #[test]
    fn eligible_pnpm_run_vitest_script() {
        assert!(eligible(
            "pnpm",
            &["run".into(), "test:vitest".into()]
        ));
    }

    #[test]
    fn not_eligible_npm_install() {
        assert!(!eligible("npm", &["install".into()]));
    }

    #[test]
    fn eligible_npx_vitest() {
        assert!(eligible("npx", &["vitest".into(), "run".into()]));
    }

    #[test]
    fn eligible_corepack_yarn_test() {
        assert!(eligible("corepack", &["yarn".into(), "test".into()]));
    }

    #[test]
    fn eligible_deno_test_cmd() {
        assert!(eligible("deno", &["test".into(), "-A".into()]));
    }

    #[test]
    fn eligible_bunx_vitest() {
        assert!(eligible("bunx", &["vitest".into(), "run".into()]));
    }

    #[test]
    fn eligible_bun_run_test_script() {
        assert!(eligible("bun", &["run".into(), "test:unit".into()]));
    }

    #[test]
    fn not_eligible_bun_install() {
        assert!(!eligible("bun", &["install".into()]));
    }

    #[test]
    fn elides_deno_ok_line() {
        assert!(should_elide_pass_noise(
            "test scope name ... ok (12ms)"
        ));
    }

    #[test]
    fn elides_bun_pass_line() {
        assert!(should_elide_pass_noise("(pass) describe > it"));
    }

    #[test]
    fn elides_vitest_check() {
        assert!(should_elide_pass_noise(" ✓ foo > bar 1ms"));
        assert!(!should_elide_pass_noise(" ✖ foo > bar FAILED"));
    }

    #[test]
    fn eligible_jest_bin() {
        assert!(eligible("jest", &[]));
        assert!(!eligible("jest", &["--help".into()]));
        assert!(!eligible("vitest", &["--version".into()]));
    }

    #[test]
    fn eligible_playwright_test_subcommand() {
        assert!(eligible(
            "playwright",
            &["test".into(), "e2e/".into()]
        ));
        assert!(!eligible(
            "playwright",
            &["show-report".into()]
        ));
    }

    #[test]
    fn eligible_cypress_run() {
        assert!(eligible("cypress", &["run".into()]));
        assert!(!eligible("cypress", &["open".into()]));
    }
}
