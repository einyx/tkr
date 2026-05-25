//! Native **pytest** — elide verbose per-test `PASSED` lines and long dot-progress rows.
//! Env: `JKR_NATIVE_PYTEST=0` disables.
//!
//! Matches `pytest` / `py.test`, `python … -m pytest`, `uv run pytest`, and
//! `poetry` / `pipenv` / `pdm run pytest`.

use super::NativeOutcome;
use crate::runner::stream_command;
use crate::stream::PipelineResult;
use anyhow::Result;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

pub fn env_disabled() -> bool {
    matches!(
        std::env::var("JKR_NATIVE_PYTEST").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

/// `path.py::…::test_name PASSED [NN%] (0.01s)` (pytest `-v`).
static VERBOSE_PASSED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^.+::.+\s+PASSED(?:\s+\[\s*\d+%\])?(?:\s*\(\d+\.\d+s\))?\s*$",
    )
    .expect("pytest verbose PASSED regex")
});

/// `tests/foo.py ......................                                    [ 80%]`
static DOT_PROGRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[^\s:]+\.py\s+\.{3,}\s+\[\s*\d+%\s*\]\s*$").expect("pytest dots regex")
});

static ANSI_ESC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:]*[a-zA-Z]").expect("ansi regex"));

fn strip_ansi(s: &str) -> String {
    ANSI_ESC.replace_all(s, "").into_owned()
}

fn strip_exe_suffix(s: &str) -> &str {
    s.strip_suffix(".exe")
        .or_else(|| s.strip_suffix(".EXE"))
        .unwrap_or(s)
}

fn normalized_base(cmd: &str) -> String {
    let mut b = Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd);
    b = strip_exe_suffix(b);
    b.to_ascii_lowercase()
}

fn is_python_interpreter(base: &str) -> bool {
    base == "python" || base == "py" || base.starts_with("python")
}

fn has_m_pytest(args: &[String]) -> bool {
    args.windows(2)
        .any(|w| w[0] == "-m" && w[1] == "pytest")
}

fn uv_run_invokes_pytest(args: &[String]) -> bool {
    if args.first().map(|s| s.as_str()) != Some("run") {
        return false;
    }
    let tail = &args[1..];
    if tail.first().map(|s| s.as_str()) == Some("pytest") {
        return true;
    }
    tail.windows(2)
        .any(|w| w[0] == "-m" && w[1] == "pytest")
}

fn tool_run_invokes_pytest(args: &[String]) -> bool {
    if args.len() < 2 || args[0] != "run" {
        return false;
    }
    if args[1] == "pytest" {
        return true;
    }
    if args.len() >= 4
        && args[1].starts_with("python")
        && args[2] == "-m"
        && args[3] == "pytest"
    {
        return true;
    }
    false
}

fn eligible(cmd: &str, args: &[String]) -> bool {
    let base = normalized_base(cmd);
    match base.as_str() {
        "pytest" | "py.test" => true,
        b if is_python_interpreter(b) => has_m_pytest(args),
        "uv" => uv_run_invokes_pytest(args),
        "poetry" | "pipenv" | "pdm" => tool_run_invokes_pytest(args),
        _ => false,
    }
}

fn should_elide_pass_noise(line: &str) -> bool {
    let plain = strip_ansi(line);
    let up = plain.to_ascii_uppercase();
    if up.contains("FAILED") || up.contains("ERROR") {
        return false;
    }
    if up.contains("XFAIL") || up.contains("XPASS") || up.contains("SKIP") {
        return false;
    }
    VERBOSE_PASSED.is_match(&plain) || DOT_PROGRESS.is_match(&plain)
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
                eprintln!("jkr: native pytest: {e}");
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
                "… ({} pytest pass-style lines elided — set `JKR_NATIVE_PYTEST=0` for full log)\n",
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
        let msg = format!("… ({} pytest pass-style lines elided)\n", pass_elided);
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

/// Cheap gate for `try_run` so unrelated commands never enter [`run`].
pub fn might_be_invocation(cmd: &str) -> bool {
    let b = normalized_base(cmd);
    matches!(
        b.as_str(),
        "pytest" | "py.test" | "uv" | "poetry" | "pipenv" | "pdm" | "py"
    ) || b.starts_with("python")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_pytest_bin() {
        assert!(eligible("pytest", &["-q".into(), "tests/".into()]));
    }

    #[test]
    fn eligible_python_m_pytest() {
        assert!(eligible(
            "python3",
            &["-W".into(), "ignore".into(), "-m".into(), "pytest".into()]
        ));
    }

    #[test]
    fn not_eligible_python_other_module() {
        assert!(!eligible("python", &["-m".into(), "http.server".into()]));
    }

    #[test]
    fn eligible_uv_run_pytest() {
        assert!(eligible("uv", &["run".into(), "pytest".into(), "-q".into()]));
    }

    #[test]
    fn eligible_poetry_run_pytest() {
        assert!(eligible(
            "poetry",
            &["run".into(), "pytest".into()]
        ));
    }

    #[test]
    fn elides_verbose_passed() {
        assert!(should_elide_pass_noise(
            "tests/test_x.py::test_a PASSED [ 50%]"
        ));
        assert!(!should_elide_pass_noise(
            "tests/test_x.py::test_a FAILED [ 50%]"
        ));
    }

    #[test]
    fn elides_dots_line() {
        assert!(should_elide_pass_noise(
            "pkg/tests/test_foo.py ..............................                    [100%]"
        ));
    }

    #[test]
    fn keeps_skipped() {
        assert!(!should_elide_pass_noise(
            "tests/z.py::t SKIPPED [1]"
        ));
    }
}
