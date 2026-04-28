use anyhow::Result;
use tkr_api::{FilterResult, Plugin};

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
                _ => {
                    // unknown escape — drop ESC, keep next as-is
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

pub fn run_pipeline<I>(lines: I, chain: &mut [Box<dyn Plugin>], command: &str, args: &str) -> PipelineResult
where I: Iterator<Item = Result<String>>,
{
    let mut emitted = Vec::new();
    let mut chars_in: u64 = 0;
    let mut chars_suppressed: u64 = 0;

    for (index, line_result) in lines.enumerate() {
        let raw = match line_result {
            Ok(l) => l,
            Err(e) => { eprintln!("tkr: read error: {e}"); continue; }
        };
        chars_in += raw.len() as u64;

        // Strip ANSI escapes before any plugin sees the line — saves tokens
        // and lets regex rules match on clean text.
        let line = strip_ansi(&raw);
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
        } else {
            println!("{current}");
            emitted.push(current);
        }
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
}
