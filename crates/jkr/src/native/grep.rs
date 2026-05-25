//! Native grep/rg: stream stdout (bounded memory), optional small-output passthrough.
//! Aligns compressed output defaults with `filters/grep.toml`.
//! Env: `TKR_NATIVE_GREP`, `TKR_GREP_NATIVE_*`, `TKR_GREP_NATIVE_RAW_MAX`.
//! For **`rg --json`** / **`--json-lines`**, parses the JSON match stream into the same grouped summary as text mode.

use super::NativeOutcome;
use crate::stream::PipelineResult;
use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

/// `TKR_NATIVE_GREP=0` → fall back to TOML line filters only.
pub fn env_disabled() -> bool {
    matches!(
        std::env::var("TKR_NATIVE_GREP").ok().as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

struct Limits {
    max_results: usize,
    max_per_file: usize,
    max_line_len: usize,
}

impl Limits {
    fn from_env() -> Self {
        Self {
            max_results: parse_usize_env("TKR_GREP_NATIVE_MAX_RESULTS", 50),
            max_per_file: parse_usize_env("TKR_GREP_NATIVE_PER_FILE", 2),
            max_line_len: parse_usize_env("TKR_GREP_NATIVE_MAX_LINE", 200),
        }
    }
}

fn parse_usize_env(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(default)
}

/// `TKR_GREP_NATIVE_RAW_MAX` — if total stdout stays under this many bytes, print
/// raw grep output (no grouping). `0` = always run structured compression.
fn parse_raw_max() -> usize {
    std::env::var("TKR_GREP_NATIVE_RAW_MAX")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8192)
}

static SUPPRESS_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(^|[/\\])(node_modules|target|dist|build|coverage|vendor|venv|htmlcov|__pycache__|buck-out)([/\\]|:)|",
        r"(^|[/\\])\.(git|next|cache|venv|tox|nyc_output|pytest_cache|mypy_cache|ruff_cache|gradle|idea|vscode|eggs)([/\\]|:)|",
        r"^Binary file .* matches$|^grep: .*: Permission denied$|^grep: .*: No such file or directory$|^grep: .*: Is a directory$"
    ))
    .expect("valid suppress regex")
});

/// `path:line:col:text` (ripgrep `--column`, unix paths without `:` in filename).
static RG_UNIX_4: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([^:\n]+):(\d+):(\d+):(.*)$").expect("rg unix4")
});

/// Standard `path:line:text` when the path has no colon (unix).
static RG_UNIX_3: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([^:\n]+):(\d+):(.*)$").expect("rg unix3")
});

/// Windows `C:\path\file.rs:line:col:text`.
static RG_WIN_4: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z]:[^:]+):(\d+):(\d+):(.*)$").expect("rg win4")
});

/// Windows `C:\path\file.rs:line:text`.
static RG_WIN_3: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z]:[^:]+):(\d+):(.*)$").expect("rg win3")
});

/// Parse a single rg/grep output line into `(path, line_no, content slice)`.
fn parse_grep_hit(line: &str) -> Option<(String, usize, &str)> {
    if let Some(c) = RG_WIN_4.captures(line) {
        let path = c.get(1)?.as_str().to_string();
        let ln: usize = c.get(2)?.as_str().parse().ok()?;
        let content = c.get(4)?.as_str();
        return Some((path, ln, content));
    }
    if let Some(c) = RG_UNIX_4.captures(line) {
        let path = c.get(1)?.as_str().to_string();
        let ln: usize = c.get(2)?.as_str().parse().ok()?;
        let content = c.get(4)?.as_str();
        return Some((path, ln, content));
    }
    if let Some(c) = RG_WIN_3.captures(line) {
        let path = c.get(1)?.as_str().to_string();
        let ln: usize = c.get(2)?.as_str().parse().ok()?;
        let content = c.get(3)?.as_str();
        return Some((path, ln, content));
    }
    if let Some(c) = RG_UNIX_3.captures(line) {
        let path = c.get(1)?.as_str().to_string();
        let ln: usize = c.get(2)?.as_str().parse().ok()?;
        let content = c.get(3)?.as_str();
        return Some((path, ln, content));
    }
    let parts: Vec<&str> = line.splitn(3, ':').collect();
    if parts.len() == 2 {
        let ln = parts[0].parse().ok()?;
        return Some((".".to_string(), ln, parts[1]));
    }
    None
}

fn wants_rg_json_args(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "--json" || a.starts_with("--json=") || a == "--json-lines"
    })
}

fn is_rg_json_mode(cmd: &str, args: &[String]) -> bool {
    Path::new(cmd)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|b| b.eq_ignore_ascii_case("rg") || b.eq_ignore_ascii_case("rg.exe"))
        .unwrap_or(false)
        && wants_rg_json_args(args)
}

fn json_match_to_grep_line(v: &Value) -> Option<String> {
    if v.get("type")?.as_str()? != "match" {
        return None;
    }
    let data = v.get("data")?;
    let path = data.pointer("/path/text")?.as_str()?;
    let line_no = data.get("line_number")?.as_u64()? as usize;
    let text = data
        .pointer("/lines/text")?
        .as_str()?
        .trim_end_matches('\n');
    Some(format!("{path}:{line_no}:{text}"))
}

fn run_rg_json(cmd: &str, cmd_args: &[String], limits: &Limits) -> Result<Option<NativeOutcome>> {
    let mut child = Command::new(cmd)
        .args(cmd_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning `{cmd}` (native rg --json)"))?;

    let stderr = child.stderr.take().expect("piped");
    let stderr_handle = std::thread::spawn(move || -> u64 {
        let mut n = 0u64;
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(l) = line else { break };
            let _ = writeln!(std::io::stderr().lock(), "{l}");
            n += l.len() as u64 + 1;
        }
        n
    });

    let stdout = child.stdout.take().expect("piped");
    let reader = BufReader::new(stdout);

    let mut stdout_bytes: u64 = 0;
    let mut state = CompressionState::new();

    for line in reader.lines() {
        let line = line?;
        let inc = line.len() as u64 + 1;
        stdout_bytes += inc;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(synthetic) = json_match_to_grep_line(&v) {
            state.feed_line(&synthetic, limits);
        }
    }

    let status = child.wait().context("waiting for rg --json")?;
    let stderr_bytes = stderr_handle.join().unwrap_or(0);
    let code = status.code().unwrap_or(-1);
    let chars_in = stdout_bytes + stderr_bytes;

    let formatted = state.into_output(limits);

    if formatted.is_empty() {
        let msg = "0 rg JSON matches";
        println!("{}", msg);
        let emitted_len = msg.len() + 1;
        return Ok(Some(NativeOutcome {
            pipeline: PipelineResult {
                emitted: vec![msg.to_string()],
                chars_in,
                chars_suppressed: chars_in.saturating_sub(emitted_len as u64),
            },
            exit_code: code,
        }));
    }

    print!("{}", formatted);
    if !formatted.ends_with('\n') {
        println!();
    }

    let add_nl = !formatted.ends_with('\n');
    let bytes_out = formatted.len() as u64 + u64::from(add_nl);
    let chars_suppressed = chars_in.saturating_sub(bytes_out);

    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![formatted.clone()],
            chars_in,
            chars_suppressed,
        },
        exit_code: code,
    }))
}

pub fn run(cmd: &str, cmd_args: &[String]) -> Result<Option<NativeOutcome>> {
    let limits = Limits::from_env();
    if is_rg_json_mode(cmd, cmd_args) {
        return run_rg_json(cmd, cmd_args, &limits);
    }
    let raw_max = parse_raw_max();
    let force_compress = raw_max == 0;

    let mut child = Command::new(cmd)
        .args(cmd_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawning `{cmd}` (native grep handler)"))?;

    let stderr = child.stderr.take().expect("piped");
    let stderr_handle = std::thread::spawn(move || -> u64 {
        let mut n = 0u64;
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let Ok(l) = line else { break };
            let _ = writeln!(std::io::stderr().lock(), "{l}");
            n += l.len() as u64 + 1;
        }
        n
    });

    let stdout = child.stdout.take().expect("piped");
    let reader = BufReader::new(stdout);

    let mut stdout_bytes: u64 = 0;
    let mut small_buf: Vec<String> = Vec::new();
    let mut small_bytes: u64 = 0;
    let mut large: Option<CompressionState> = None;

    for line in reader.lines() {
        let line = line?;
        let inc = line.len() as u64 + 1;
        stdout_bytes += inc;

        if large.is_none() && !force_compress {
            if small_bytes + inc <= raw_max as u64 {
                small_buf.push(line);
                small_bytes += inc;
                continue;
            }
            let mut st = CompressionState::new();
            for l in small_buf.drain(..) {
                st.feed_line(&l, &limits);
            }
            st.feed_line(&line, &limits);
            large = Some(st);
            continue;
        }
        large
            .get_or_insert_with(CompressionState::new)
            .feed_line(&line, &limits);
    }

    let status = child.wait().context("waiting for grep/rg")?;
    let stderr_bytes = stderr_handle.join().unwrap_or(0);
    let code = status.code().unwrap_or(-1);
    let chars_in = stdout_bytes + stderr_bytes;

    if stdout_bytes == 0 && small_buf.is_empty() && large.is_none() {
        let msg = "0 grep matches";
        println!("{}", msg);
        let emitted_len = msg.len() + 1;
        return Ok(Some(NativeOutcome {
            pipeline: PipelineResult {
                emitted: vec![msg.to_string()],
                chars_in: chars_in as u64,
                chars_suppressed: chars_in.saturating_sub(emitted_len as u64) as u64,
            },
            exit_code: code,
        }));
    }

    let (formatted, add_nl) = if let Some(st) = large {
        let s = st.into_output(&limits);
        let nl = !s.ends_with('\n');
        (s, nl)
    } else {
        // Small path: entire stdout fit in buffer (or force_compress false and we never spilled)
        let joined = small_buf.join("\n");
        let s = if joined.ends_with('\n') {
            joined
        } else if !small_buf.is_empty() {
            format!("{joined}\n")
        } else {
            String::new()
        };
        let nl = false;
        (s, nl)
    };

    print!("{}", formatted);
    if add_nl {
        println!();
    }

    let bytes_out = formatted.len() as u64 + u64::from(add_nl);
    let chars_suppressed = chars_in.saturating_sub(bytes_out);

    Ok(Some(NativeOutcome {
        pipeline: PipelineResult {
            emitted: vec![formatted.clone()],
            chars_in: chars_in as u64,
            chars_suppressed,
        },
        exit_code: code,
    }))
}

#[derive(Default)]
struct CompressionState {
    by_file: HashMap<String, Vec<(usize, String)>>,
    /// Non-empty, non-suppressed lines we attempted to parse as hits.
    match_line_count: usize,
}

impl CompressionState {
    fn new() -> Self {
        Self::default()
    }

    fn feed_line(&mut self, line: &str, limits: &Limits) {
        if line.trim().is_empty() || should_drop_line(line) {
            return;
        }
        let Some((file, line_num, content)) = parse_grep_hit(line) else {
            return;
        };
        self.match_line_count += 1;
        let cleaned = truncate_line(content.trim(), limits.max_line_len);
        self.by_file.entry(file).or_default().push((line_num, cleaned));
    }

    fn into_output(self, limits: &Limits) -> String {
        if self.by_file.is_empty() {
            return String::new();
        }
        let total_matches = self.match_line_count;
        let mut files: Vec<_> = self.by_file.keys().cloned().collect();
        files.sort();

        let mut out = String::new();
        out.push_str(&format!(
            "{} matches in {} files:\n\n",
            total_matches,
            self.by_file.len()
        ));

        let mut shown = 0usize;
        for file in files {
            if shown >= limits.max_results {
                break;
            }
            let matches = self.by_file.get(&file).expect("key");
            let file_display = compact_path(&file);
            for (line_num, content) in matches.iter().take(limits.max_per_file) {
                if shown >= limits.max_results {
                    break;
                }
                out.push_str(&format!("{}:{}:{}\n", file_display, line_num, content));
                shown += 1;
            }
        }

        if total_matches > shown {
            out.push_str(&format!(
                "[+{} more]\n",
                total_matches.saturating_sub(shown)
            ));
        }

        out
    }
}

fn should_drop_line(line: &str) -> bool {
    SUPPRESS_LINE.is_match(line)
}

fn truncate_line(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t = String::new();
    for c in s.chars().take(max.saturating_sub(3)) {
        t.push(c);
    }
    t.push_str("...");
    t
}

fn compact_path(path: &str) -> String {
    if path.len() <= 50 {
        return path.to_string();
    }
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = path.split(sep).collect();
    if parts.len() <= 3 {
        return path.to_string();
    }
    format!(
        "{}{sep}...{sep}{}{sep}{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1],
        sep = sep
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_groups_and_caps() {
        let raw = "\
src/a.rs:1:fn one()
src/a.rs:2:fn two()
src/a.rs:3:fn three()
src/b.rs:10:fn b()
";
        let limits = Limits {
            max_results: 50,
            max_per_file: 2,
            max_line_len: 200,
        };
        let mut st = CompressionState::new();
        for l in raw.lines() {
            st.feed_line(l, &limits);
        }
        let s = st.into_output(&limits);
        assert!(s.contains("matches"));
        assert!(s.contains("src/a.rs:1:"));
        assert!(s.contains("src/a.rs:2:"));
        assert!(!s.contains("src/a.rs:3:"));
        assert!(s.contains("src/b.rs"));
    }

    #[test]
    fn parses_windows_drive_path() {
        let line = r"C:\proj\crates\tkr\src\lib.rs:42:pub fn x()";
        let t = parse_grep_hit(line).expect("parse");
        assert!(t.0.starts_with("C:"));
        assert_eq!(t.1, 42);
        assert!(t.2.contains("pub fn"));
    }

    #[test]
    fn parses_unix_with_column() {
        let line = "crates/tkr/src/lib.rs:10:3:pub struct";
        let t = parse_grep_hit(line).expect("parse");
        assert_eq!(t.1, 10);
        assert_eq!(t.2, "pub struct");
    }

    #[test]
    fn drops_node_modules_line() {
        let raw = "node_modules/foo:1:bad()\nsrc/x.rs:1:good()\n";
        let limits = Limits {
            max_results: 50,
            max_per_file: 5,
            max_line_len: 200,
        };
        let mut st = CompressionState::new();
        for l in raw.lines() {
            st.feed_line(l, &limits);
        }
        let s = st.into_output(&limits);
        assert!(!s.contains("node_modules"));
        assert!(s.contains("src/x.rs"));
    }

    #[test]
    fn rg_json_match_maps_to_compression() {
        let raw = r#"{"type":"match","data":{"path":{"text":"src/a.rs"},"lines":{"text":"fn z()\n"},"line_number":9}}"#;
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        let synthetic = json_match_to_grep_line(&v).expect("line");
        let limits = Limits {
            max_results: 50,
            max_per_file: 5,
            max_line_len: 200,
        };
        let mut st = CompressionState::new();
        st.feed_line(&synthetic, &limits);
        let out = st.into_output(&limits);
        assert!(out.contains("src/a.rs:9:"));
    }

    #[test]
    fn wants_json_flag() {
        assert!(wants_rg_json_args(&["--json".into(), "p".into(), ".".into()]));
        assert!(!wants_rg_json_args(&["pattern".into(), ".".into()]));
    }
}
