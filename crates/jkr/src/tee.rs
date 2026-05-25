//! Save **unfiltered** merged stdout/stderr transcripts under `~/.jkr/tee/`,
//! RTK-style, so agents can open full logs after filtered output without
//! re-running the command (especially on failures).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound on transcript bytes we retain in memory / write (default 8 MiB).
fn max_bytes() -> usize {
    std::env::var("JKR_TEE_MAX_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 1024)
        .unwrap_or(8 * 1024 * 1024)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeeMode {
    Never,
    Failures,
    Always,
}

pub fn tee_mode() -> TeeMode {
    match std::env::var("JKR_TEE").ok().as_deref() {
        None | Some("") => TeeMode::Failures,
        Some(s) => {
            let t = s.trim().to_ascii_lowercase();
            match t.as_str() {
                "0" | "false" | "off" | "no" | "never" => TeeMode::Never,
                "always" | "all" => TeeMode::Always,
                "failures" | "failure" | "1" | "true" | "on" | "yes" => TeeMode::Failures,
                _ => TeeMode::Failures,
            }
        }
    }
}

/// Whether we should append each raw line into an in-memory transcript.
pub fn capture_raw_transcript() -> bool {
    !matches!(tee_mode(), TeeMode::Never)
}

fn should_write_disk(mode: TeeMode, exit_code: i32) -> bool {
    match mode {
        TeeMode::Never => false,
        TeeMode::Always => true,
        TeeMode::Failures => exit_code != 0,
    }
}

fn tee_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let dir = home.join(".jkr").join("tee");
    Some(dir)
}

fn sanitize_slug(command: &str, args: &str) -> String {
    let mut s = format!("{command}");
    if !args.is_empty() {
        s.push('_');
        s.push_str(args);
    }
    let max = 96usize;
    if s.len() > max {
        s.truncate(max);
    }
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn transcript_cap() -> usize {
    max_bytes()
}

/// Append one raw line (no trailing newline in `line`) plus `\n`, respecting size cap.
pub fn append_raw_line(buf: &mut String, line: &str, cap: usize) {
    if buf.len() >= cap {
        return;
    }
    const MARKER: &str = "[jkr: tee truncated — JKR_TEE_MAX_BYTES]\n";
    let need = line.len().saturating_add(1);
    let room = cap.saturating_sub(buf.len());
    if need <= room {
        buf.push_str(line);
        buf.push('\n');
        return;
    }
    if room <= 1 {
        return;
    }
    let prefix_bytes = room.saturating_sub(1);
    let prefix = truncate_utf8_prefix(line, prefix_bytes);
    buf.push_str(prefix);
    buf.push('\n');
    if buf.len().saturating_add(MARKER.len()) <= cap {
        buf.push_str(MARKER);
    }
}

fn truncate_utf8_prefix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// On matching tee mode, write `transcript` and return a single-line hint for stdout.
pub fn maybe_save_transcript(
    command: &str,
    args: &str,
    exit_code: i32,
    transcript: &str,
) -> std::io::Result<Option<String>> {
    let mode = tee_mode();
    if !should_write_disk(mode, exit_code) {
        return Ok(None);
    }
    if transcript.trim().is_empty() {
        return Ok(None);
    }

    let Some(dir) = tee_dir() else {
        return Ok(None);
    };
    std::fs::create_dir_all(&dir)?;

    let slug = sanitize_slug(command, args);
    let path = dir.join(format!("{}_{}.log", unix_secs(), slug));

    let meta = format!(
        "# jkr tee · exit={exit_code} · cmd={command} {args}\n",
        args = args,
        command = command,
        exit_code = exit_code
    );
    atomic_write(&path, &format!("{meta}{transcript}"))?;

    Ok(Some(format!(
        "[jkr: full output saved to {}]",
        path.display()
    )))
}

fn atomic_write(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tee.tmp");
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_slug_ok() {
        assert_eq!(
            sanitize_slug("cargo", "test -q"),
            "cargo_test__q"
        );
    }

    #[test]
    fn append_respects_cap() {
        let mut s = String::new();
        append_raw_line(&mut s, "hello", 100);
        assert_eq!(s, "hello\n");
        let mut big = String::new();
        append_raw_line(&mut big, &"x".repeat(200), 80);
        assert!(big.len() <= 80 + 60);
        assert!(big.contains('\n'));
    }
}
