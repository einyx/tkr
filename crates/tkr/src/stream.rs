use anyhow::Result;
use std::cell::RefCell;
use std::collections::HashMap;
use tkr_api::{FilterResult, LegacyPlugin as Plugin};

use crate::signature::signature_of;
use crate::host::loader::PluginRegistry;

/// Cap on unique (command, signature) pairs held per process. Past this we stop
/// inserting new signatures (sample-and-drop) but keep updating existing ones,
/// so a single noisy run doesn't blow memory.
const SIG_BUFFER_CAP: usize = 1000;

#[derive(Default)]
pub struct SigEntry {
    pub sample: String,
    pub occurrences: u64,
    pub total_chars: u64,
}

thread_local! {
    static SIG_BUFFER: RefCell<HashMap<(String, String), SigEntry>> =
        RefCell::new(HashMap::new());
}

fn record_emit(command: &str, line: &str) {
    let sig = signature_of(line);
    SIG_BUFFER.with(|b| {
        let mut buf = b.borrow_mut();
        let key = (command.to_string(), sig);
        if let Some(entry) = buf.get_mut(&key) {
            entry.occurrences += 1;
            entry.total_chars += line.len() as u64;
            return;
        }
        if buf.len() >= SIG_BUFFER_CAP {
            return;
        }
        buf.insert(
            key,
            SigEntry {
                sample: line.to_string(),
                occurrences: 1,
                total_chars: line.len() as u64,
            },
        );
    });
}

/// Drain the per-process signature buffer. Called by the proxy at end of run
/// to flush into the analytics store in one transaction.
pub fn take_signature_buffer() -> Vec<(String, String, SigEntry)> {
    SIG_BUFFER.with(|b| {
        let mut buf = b.borrow_mut();
        let drained: HashMap<(String, String), SigEntry> = std::mem::take(&mut *buf);
        drained
            .into_iter()
            .map(|((cmd, sig), entry)| (cmd, sig, entry))
            .collect()
    })
}

pub struct PipelineResult {
    pub emitted: Vec<String>,
    pub chars_in: u64,
    pub chars_suppressed: u64,
}

/// Strip ANSI escape sequences (CSI / OSC) and standalone bell/backspace/CR.
/// Iterates over chars to preserve multi-byte UTF-8 (├ ─ ✓ etc).
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    // CSI: parameters then a final byte 0x40..=0x7E
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if (0x40u32..=0x7e).contains(&(n as u32)) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: terminated by BEL or ESC \
                    while let Some(&n) = chars.peek() {
                        if n == '\x07' {
                            chars.next();
                            break;
                        }
                        if n == '\x1b' {
                            chars.next();
                            if let Some(&'\\') = chars.peek() {
                                chars.next();
                            }
                            break;
                        }
                        chars.next();
                    }
                }
                Some(&n) if matches!(n, '(' | ')' | '*' | '+') => {
                    chars.next();
                    chars.next(); // consume designator char
                }
                Some(&n) if matches!(n, 'P' | '_' | '^' | 'X') => {
                    // DCS, APC, PM, SOS — string sequences terminated by ST (ESC \) or BEL.
                    chars.next();
                    while let Some(&m) = chars.peek() {
                        if m == '\x07' {
                            chars.next();
                            break;
                        }
                        if m == '\x1b' {
                            chars.next();
                            if let Some(&'\\') = chars.peek() {
                                chars.next();
                            }
                            break;
                        }
                        chars.next();
                    }
                }
                Some(_) => {
                    // Single-shift / two-char escape (SS2/SS3 ESC N/O, RIS ESC c, etc.).
                    chars.next();
                }
                None => {
                    // Trailing ESC — just drop it.
                }
            }
            continue;
        }
        if c == '\x07' || c == '\x08' || c == '\r' {
            continue;
        }
        out.push(c);
    }
    out
}

/// Default cap on a single line's length. Prevents one obnoxious base64/minified
/// blob from blowing context. Past this, line is truncated with an elision marker.
const MAX_LINE_LEN: usize = 800;

/// Universal per-line preprocessing applied before any plugin sees the line:
///   1. Strip ANSI escape sequences (saves all the tokens of color codes)
///   2. Shorten $HOME paths to `~` (saves ~15-25 chars on every error/trace line)
///   3. Trim trailing whitespace (lossless)
///   4. Truncate excessively long single lines
pub fn normalize_line(raw: &str) -> String {
    let s = strip_ansi(raw);
    let s = shorten_home(&s);
    let s = s.trim_end();
    truncate_long_line(s)
}

fn truncate_long_line(s: &str) -> String {
    if s.len() <= MAX_LINE_LEN {
        return s.to_string();
    }
    let elided = s.len() - MAX_LINE_LEN;
    // Use chars() to avoid mid-codepoint truncation.
    let mut head = String::with_capacity(MAX_LINE_LEN + 24);
    let mut taken = 0usize;
    for c in s.chars() {
        let w = c.len_utf8();
        if taken + w > MAX_LINE_LEN {
            break;
        }
        head.push(c);
        taken += w;
    }
    head.push_str(&format!(" … (+{} chars truncated)", elided));
    head
}

/// Replace occurrences of $HOME with `~` so `/Users/alice/proj/foo` → `~/proj/foo`.
/// Avoids tokens that LLMs don't need (the user's literal home path).
fn shorten_home(line: &str) -> String {
    let home = match dirs::home_dir() {
        Some(p) => p.to_string_lossy().into_owned(),
        None => return line.to_string(),
    };
    if home.is_empty() || !line.contains(&home) {
        return line.to_string();
    }
    line.replace(&home, "~")
}

pub fn run_pipeline<I>(lines: I, chain: &mut [Box<dyn Plugin>], command: &str, args: &str) -> PipelineResult
where I: Iterator<Item = Result<String>>,
{
    let mut emitted = Vec::new();
    let mut chars_in: u64 = 0;
    let mut chars_suppressed: u64 = 0;
    let mut blank_run: u32 = 0;
    // --max-tokens budget. None = unlimited. Read once at start.
    let max_tokens = std::env::var("TKR_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let mut emitted_chars: u64 = 0;
    let mut budget_exceeded = false;
    let mut elided_lines: u64 = 0;

    for (index, line_result) in lines.enumerate() {
        let raw = match line_result {
            Ok(l) => l,
            Err(e) => { eprintln!("tkr: read error: {e}"); continue; }
        };
        chars_in += raw.len() as u64;

        // Pre-process: strip ANSI, shorten $HOME paths, trim trailing whitespace,
        // truncate excessively long single lines.
        let line = normalize_line(&raw);

        // Anything dropped by normalize (mostly: huge-line truncation, ANSI codes,
        // trailing whitespace) counts as savings.
        if raw.len() > line.len() {
            chars_suppressed += (raw.len() - line.len()) as u64;
        }

        // Collapse runs of blank lines: keep at most ONE blank between content.
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                chars_suppressed += line.len() as u64;
                continue;
            }
        } else {
            blank_run = 0;
        }

        let mut current = line.clone();
        let mut suppressed = false;

        for plugin in chain.iter_mut() {
            let result = plugin.filter(&current, command, args, index as u64);
            match result {
                FilterResult::Pass => {}
                FilterResult::Replace(ptr, len) => {
                    if !ptr.is_null() {
                        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
                        current = String::from_utf8_lossy(bytes).into_owned();
                    }
                }
                FilterResult::Suppress | FilterResult::SuppressWithNote(_) => {
                    suppressed = true;
                    break;
                }
                FilterResult::Annotate(ptr, len) => {
                    if !ptr.is_null() {
                        let ann = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
                        current.push(' ');
                        current.push_str(&String::from_utf8_lossy(ann));
                    }
                }
            }
        }

        if suppressed {
            chars_suppressed += line.len() as u64;
            continue;
        }

        // Filter rule shortened the line via TruncateMatch — count the diff
        // as suppressed too.
        if current.len() < line.len() {
            chars_suppressed += (line.len() - current.len()) as u64;
        }

        // --max-tokens budget — once exceeded, stop emitting and count elided.
        if let Some(max) = max_tokens {
            if budget_exceeded {
                elided_lines += 1;
                chars_suppressed += line.len() as u64;
                continue;
            }
            if chars_to_tokens(emitted_chars + current.len() as u64) > max {
                budget_exceeded = true;
                elided_lines += 1;
                chars_suppressed += line.len() as u64;
                continue;
            }
            emitted_chars += current.len() as u64;
        }

        record_emit(command, &current);
        println!("{current}");
        emitted.push(current);
    }

    if budget_exceeded {
        let msg = format!(
            "(... {} more lines elided — TKR_MAX_TOKENS={} reached)",
            elided_lines,
            max_tokens.unwrap_or(0)
        );
        println!("{}", msg);
        emitted.push(msg);
    }

    for plugin in chain.iter_mut() {
        let summary = plugin.flush();
        if !summary.is_empty() {
            for summary_line in summary.lines() {
                println!("{summary_line}");
                emitted.push(summary_line.to_string());
            }
        }
    }

    PipelineResult { emitted, chars_in, chars_suppressed }
}

/// V2 pipeline: filters via the plugin registry (v2 `on_line`), then passes
/// remaining lines through the legacy semantic chain. Analytics is NOT done
/// here — the v2 `AnalyticsPluginV2` accumulates chars via its own `on_line`.
///
/// The caller must call `registry.filters_command_begin` before this function
/// and `registry.filters_command_end` after it.
pub fn run_pipeline_v2<I>(
    lines: I,
    registry: &PluginRegistry,
    semantic_chain: &mut [Box<dyn Plugin>],
    command: &str,
    args: &str,
) -> PipelineResult
where
    I: Iterator<Item = Result<String>>,
{
    use tkr_api::plugin::CommandCtx;

    let mut emitted = Vec::new();
    let mut chars_in: u64 = 0;
    let mut chars_suppressed: u64 = 0;
    let mut blank_run: u32 = 0;
    let max_tokens = std::env::var("TKR_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let mut emitted_chars: u64 = 0;
    let mut budget_exceeded = false;
    let mut elided_lines: u64 = 0;
    // Buffered mode: defer all stdout writes until end-of-pipeline so we can
    // try to compact the whole output as JSON. Triggered by --compact-json
    // (wired via TKR_COMPACT_JSON env from main.rs).
    let buffered = std::env::var_os("TKR_COMPACT_JSON").is_some();

    for (index, line_result) in lines.enumerate() {
        let raw = match line_result {
            Ok(l) => l,
            Err(e) => { eprintln!("tkr: read error: {e}"); continue; }
        };
        chars_in += raw.len() as u64;

        let line = normalize_line(&raw);

        if raw.len() > line.len() {
            chars_suppressed += (raw.len() - line.len()) as u64;
        }

        // Collapse runs of blank lines.
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                chars_suppressed += line.len() as u64;
                continue;
            }
        } else {
            blank_run = 0;
        }

        // --- V2 filter registry ---
        let ctx = CommandCtx {
            command: command.to_string(),
            args: args.to_string(),
            line_index: index as u64,
        };
        let after_v2 = registry.run_filters_line(line.clone(), &ctx);
        let current = match after_v2 {
            None => {
                chars_suppressed += line.len() as u64;
                continue;
            }
            Some(s) => s,
        };

        // --- Legacy semantic chain ---
        let mut current = current;
        let mut suppressed = false;
        for plugin in semantic_chain.iter_mut() {
            let result = plugin.filter(&current, command, args, index as u64);
            match result {
                FilterResult::Pass => {}
                FilterResult::Replace(ptr, len) => {
                    if !ptr.is_null() {
                        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
                        current = String::from_utf8_lossy(bytes).into_owned();
                    }
                }
                FilterResult::Suppress | FilterResult::SuppressWithNote(_) => {
                    suppressed = true;
                    break;
                }
                FilterResult::Annotate(ptr, len) => {
                    if !ptr.is_null() {
                        let ann = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
                        current.push(' ');
                        current.push_str(&String::from_utf8_lossy(ann));
                    }
                }
            }
        }

        if suppressed {
            chars_suppressed += line.len() as u64;
            continue;
        }

        if current.len() < line.len() {
            chars_suppressed += (line.len() - current.len()) as u64;
        }

        // --max-tokens budget.
        if let Some(max) = max_tokens {
            if budget_exceeded {
                elided_lines += 1;
                chars_suppressed += line.len() as u64;
                continue;
            }
            if chars_to_tokens(emitted_chars + current.len() as u64) > max {
                budget_exceeded = true;
                elided_lines += 1;
                chars_suppressed += line.len() as u64;
                continue;
            }
            emitted_chars += current.len() as u64;
        }

        record_emit(command, &current);
        if !buffered {
            println!("{current}");
        }
        emitted.push(current);
    }

    if budget_exceeded {
        let msg = format!(
            "(... {} more lines elided — TKR_MAX_TOKENS={} reached)",
            elided_lines,
            max_tokens.unwrap_or(0)
        );
        if !buffered {
            println!("{}", msg);
        }
        emitted.push(msg);
    }

    // Capture the body length before plugin flush summaries are appended so
    // JSON compaction sees only the original output.
    let body_end = emitted.len();

    // Flush legacy semantic plugin summaries.
    for plugin in semantic_chain.iter_mut() {
        let summary = plugin.flush();
        if !summary.is_empty() {
            for summary_line in summary.lines() {
                if !buffered {
                    println!("{summary_line}");
                }
                emitted.push(summary_line.to_string());
            }
        }
    }

    if buffered {
        let body = emitted[..body_end].join("\n");
        let summaries = if emitted.len() > body_end {
            emitted[body_end..].join("\n")
        } else {
            String::new()
        };
        let final_body = compact_json_if_possible(&body).unwrap_or(body.clone());
        if final_body.len() < body.len() {
            chars_suppressed += (body.len() - final_body.len()) as u64;
        }
        println!("{final_body}");
        if !summaries.is_empty() {
            println!("{summaries}");
        }
    }

    PipelineResult { emitted, chars_in, chars_suppressed }
}

/// If `body` parses as a JSON document, re-serialize it without indentation
/// and return the compact form. Otherwise return None so the caller can emit
/// the buffered text unchanged.
fn compact_json_if_possible(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let first = trimmed.as_bytes()[0];
    if first != b'{' && first != b'[' {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    serde_json::to_string(&value).ok()
}

pub fn chars_to_tokens(chars: u64) -> u64 { chars / 4 }

#[cfg(test)]
mod tests {
    use super::*;

    struct PassPlugin;
    impl Plugin for PassPlugin {
        fn init(_: &str) -> Box<dyn Plugin> where Self: Sized { Box::new(PassPlugin) }
        fn filter(&mut self, _l: &str, _c: &str, _a: &str, _i: u64) -> FilterResult { FilterResult::Pass }
        fn flush(&mut self) -> String { String::new() }
    }

    struct SuppressAll;
    impl Plugin for SuppressAll {
        fn init(_: &str) -> Box<dyn Plugin> where Self: Sized { Box::new(SuppressAll) }
        fn filter(&mut self, _l: &str, _c: &str, _a: &str, _i: u64) -> FilterResult { FilterResult::Suppress }
        fn flush(&mut self) -> String { String::new() }
    }

    fn lines<'a>(v: &'a [&'a str]) -> impl Iterator<Item = Result<String>> + 'a {
        v.iter().map(|s| Ok(s.to_string()))
    }

    #[test]
    fn pass_through_emits_all() {
        let mut chain: Vec<Box<dyn Plugin>> = vec![Box::new(PassPlugin)];
        let r = run_pipeline(lines(&["a", "b", "c"]), &mut chain, "git", "status");
        assert_eq!(r.emitted.len(), 3);
        assert_eq!(r.chars_suppressed, 0);
    }

    #[test]
    fn suppress_all_emits_none() {
        let mut chain: Vec<Box<dyn Plugin>> = vec![Box::new(SuppressAll)];
        let r = run_pipeline(lines(&["a", "b"]), &mut chain, "git", "status");
        assert_eq!(r.emitted.len(), 0);
        assert_eq!(r.chars_suppressed, 2);
    }

    #[test]
    fn tokens_approx() { assert_eq!(chars_to_tokens(400), 100); }

    #[test]
    fn strip_ansi_removes_color_codes() {
        let input = "\x1b[1;31mERROR\x1b[0m: oops";
        assert_eq!(strip_ansi(input), "ERROR: oops");
    }

    #[test]
    fn strip_ansi_passes_plain_text() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn strip_ansi_drops_carriage_returns() {
        assert_eq!(strip_ansi("progress\rdone"), "progressdone");
    }

    #[test]
    fn strip_ansi_handles_osc_titles() {
        let input = "\x1b]0;tab title\x07actual content";
        assert_eq!(strip_ansi(input), "actual content");
    }

    #[test]
    fn strip_ansi_preserves_utf8_box_drawing() {
        // Regression: bug where strip_ansi iterated bytes and corrupted UTF-8.
        let input = "├── \x1b[1manyhow\x1b[0m v1.0.102";
        assert_eq!(strip_ansi(input), "├── anyhow v1.0.102");
    }

    #[test]
    fn normalize_strips_ansi_and_trailing_ws() {
        assert_eq!(normalize_line("\x1b[1mhello\x1b[0m   \t  "), "hello");
    }

    #[test]
    fn shorten_home_no_op_when_no_home_in_line() {
        assert_eq!(shorten_home("just text"), "just text");
    }

    #[test]
    fn strip_ansi_handles_dcs_string() {
        // DCS ESC P ... ST. Body should be dropped, surrounding text kept.
        let input = "before\x1bP1$rmsg\x1b\\after";
        assert_eq!(strip_ansi(input), "beforeafter");
    }
}
