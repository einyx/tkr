use regex::Regex;
use serde::Deserialize;
use tkr_api::FilterResult;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Rule {
    SuppressPrefix {
        prefix: String,
    },
    SuppressRegex {
        pattern: String,
    },
    KeepRegex {
        pattern: String,
    },
    /// Suppress consecutive lines that all match `pattern`, after the first one.
    /// E.g. for git log, `^Author: ` collapses repeated authors when same.
    /// `match_capture` (optional): if set, only collapse when that capture group
    /// has the same value as the previous match. Group `1` by default.
    CollapseRepeats {
        pattern: String,
        #[serde(default)]
        match_capture: Option<usize>,
    },
    /// Replace each match of `pattern` with `replace` (regex replacement
    /// template — supports `$1`, `$2`, `$name`). Useful for shortening
    /// long SHAs, paths, or date timestamps without losing the line.
    /// Only emits a Replace result if the replacement actually changed the line.
    TruncateMatch {
        pattern: String,
        replace: String,
    },
    /// Suppress consecutive matches of `pattern` past the first `keep_first`
    /// lines (default 3). For runs of unchanged context in diffs, repetitive
    /// log lines, etc. The first `keep_first` matching lines pass through;
    /// subsequent ones are suppressed until a non-matching line ends the run.
    /// `ignore_blanks` (default false): if true, blank lines (after trim)
    /// neither match nor reset — they pass through and the run continues.
    /// Useful for multi-paragraph blocks like commit-message bodies.
    CollapseRun {
        pattern: String,
        #[serde(default = "default_keep_first")]
        keep_first: u32,
        #[serde(default)]
        ignore_blanks: bool,
    },
    /// Suppress consecutive lines sharing the same first `prefix_len` chars
    /// past the first `keep_first` (default 1). Useful for log lines like
    /// `[2026-04-28T21:00:00] req=abc123 ...` that share a long timestamp prefix.
    CollapseCommonPrefix {
        #[serde(default = "default_prefix_len")]
        prefix_len: usize,
        #[serde(default = "default_keep_first")]
        keep_first: u32,
    },
}

fn default_prefix_len() -> usize {
    24
}

fn default_keep_first() -> u32 {
    3
}

pub struct CompiledRule {
    pub kind: RuleKind,
    pub state: RuleState,
}

pub enum RuleKind {
    SuppressPrefix(String),
    SuppressRegex(Regex),
    KeepRegex(Regex),
    CollapseRepeats(Regex, Option<usize>),
    TruncateMatch(Regex, String),
    CollapseRun(Regex, u32, bool),
    CollapseCommonPrefix(usize, u32),
}

pub enum RuleState {
    None,
    LastCapture(Option<String>),
    /// Number of consecutive matches accumulated for CollapseRun.
    RunCount(u32),
    /// (last_prefix, count) for CollapseCommonPrefix
    PrefixRun(Option<String>, u32),
}

impl Rule {
    pub fn compile(self) -> anyhow::Result<CompiledRule> {
        match self {
            Rule::SuppressPrefix { prefix } => Ok(CompiledRule {
                kind: RuleKind::SuppressPrefix(prefix),
                state: RuleState::None,
            }),
            Rule::SuppressRegex { pattern } => Ok(CompiledRule {
                kind: RuleKind::SuppressRegex(Regex::new(&pattern)?),
                state: RuleState::None,
            }),
            Rule::KeepRegex { pattern } => Ok(CompiledRule {
                kind: RuleKind::KeepRegex(Regex::new(&pattern)?),
                state: RuleState::None,
            }),
            Rule::CollapseRepeats {
                pattern,
                match_capture,
            } => Ok(CompiledRule {
                kind: RuleKind::CollapseRepeats(Regex::new(&pattern)?, match_capture),
                state: RuleState::LastCapture(None),
            }),
            Rule::TruncateMatch { pattern, replace } => Ok(CompiledRule {
                kind: RuleKind::TruncateMatch(Regex::new(&pattern)?, replace),
                state: RuleState::None,
            }),
            Rule::CollapseRun {
                pattern,
                keep_first,
                ignore_blanks,
            } => Ok(CompiledRule {
                kind: RuleKind::CollapseRun(Regex::new(&pattern)?, keep_first, ignore_blanks),
                state: RuleState::RunCount(0),
            }),
            Rule::CollapseCommonPrefix {
                prefix_len,
                keep_first,
            } => Ok(CompiledRule {
                kind: RuleKind::CollapseCommonPrefix(prefix_len, keep_first),
                state: RuleState::PrefixRun(None, 0),
            }),
        }
    }
}

impl CompiledRule {
    /// Returns `Some(FilterResult)` if this rule has an opinion, `None` to defer.
    pub fn apply(&mut self, line: &str) -> Option<FilterResult> {
        match &self.kind {
            RuleKind::SuppressPrefix(p) => {
                if line.starts_with(p.as_str()) {
                    Some(FilterResult::Suppress)
                } else {
                    None
                }
            }
            RuleKind::SuppressRegex(re) => {
                if re.is_match(line) {
                    Some(FilterResult::Suppress)
                } else {
                    None
                }
            }
            RuleKind::KeepRegex(re) => {
                if re.is_match(line) {
                    None
                } else {
                    Some(FilterResult::Suppress)
                }
            }
            RuleKind::TruncateMatch(re, replace) => {
                let new = re.replace_all(line, replace.as_str());
                if new == line {
                    None
                } else {
                    // Leak the new bytes into a stable pointer/len that the
                    // stream pipeline can copy into its own String. The leak
                    // is bounded by the number of matched lines in a single
                    // command run.
                    let bytes = new.into_owned().into_bytes().into_boxed_slice();
                    let len = bytes.len();
                    let ptr = Box::leak(bytes).as_mut_ptr() as *mut std::os::raw::c_char;
                    Some(FilterResult::Replace(ptr, len))
                }
            }
            RuleKind::CollapseCommonPrefix(prefix_len, keep_first) => {
                if line.len() < *prefix_len {
                    self.state = RuleState::PrefixRun(None, 0);
                    return None;
                }
                let cur_prefix: String = line.chars().take(*prefix_len).collect();
                let new_count = match &self.state {
                    RuleState::PrefixRun(Some(p), n) if p == &cur_prefix => n + 1,
                    _ => 1,
                };
                self.state = RuleState::PrefixRun(Some(cur_prefix), new_count);
                if new_count > *keep_first {
                    Some(FilterResult::SuppressWithNote(0))
                } else {
                    None
                }
            }
            RuleKind::CollapseRun(re, keep_first, ignore_blanks) => {
                if re.is_match(line) {
                    let count = match self.state {
                        RuleState::RunCount(n) => n + 1,
                        _ => 1,
                    };
                    self.state = RuleState::RunCount(count);
                    if count > *keep_first {
                        Some(FilterResult::SuppressWithNote(0))
                    } else {
                        None
                    }
                } else if *ignore_blanks && line.trim().is_empty() {
                    // Blank line — neither matches nor resets the run.
                    None
                } else {
                    self.state = RuleState::RunCount(0);
                    None
                }
            }
            RuleKind::CollapseRepeats(re, group) => {
                if let Some(caps) = re.captures(line) {
                    let group_idx = group.unwrap_or(1);
                    let current = caps
                        .get(group_idx)
                        .or_else(|| caps.get(0))
                        .map(|m| m.as_str().to_string());
                    let suppress = match (&self.state, &current) {
                        (RuleState::LastCapture(Some(prev)), Some(now)) => prev == now,
                        _ => false,
                    };
                    self.state = RuleState::LastCapture(current);
                    if suppress {
                        Some(FilterResult::Suppress)
                    } else {
                        None
                    }
                } else {
                    // Line doesn't match this rule's pattern — reset state
                    // so a later identical line doesn't get collapsed across
                    // unrelated content.
                    self.state = RuleState::LastCapture(None);
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_repeats_drops_consecutive_same_capture() {
        let rule = Rule::CollapseRepeats {
            pattern: r"^Author: (.+)$".to_string(),
            match_capture: Some(1),
        };
        let mut compiled = rule.compile().unwrap();
        // First Author: alice — kept (no prior)
        assert_eq!(compiled.apply("Author: alice"), None);
        // Same author — suppress
        assert_eq!(
            compiled.apply("Author: alice"),
            Some(FilterResult::Suppress)
        );
        // Different author — kept
        assert_eq!(compiled.apply("Author: bob"), None);
        // Same again — suppress
        assert_eq!(compiled.apply("Author: bob"), Some(FilterResult::Suppress));
    }

    #[test]
    fn truncate_match_shortens_sha() {
        let rule = Rule::TruncateMatch {
            pattern: r"^commit ([0-9a-f]{7})[0-9a-f]{33}".to_string(),
            replace: "commit $1".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        let result = compiled.apply("commit 81cda431a68d54a55707179f546a4b6c449e92e2");
        match result {
            Some(FilterResult::Replace(ptr, len)) => {
                let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
                let s = std::str::from_utf8(bytes).unwrap();
                assert_eq!(s, "commit 81cda43");
            }
            other => panic!("expected Replace, got {:?}", other),
        }
    }

    #[test]
    fn collapse_run_keeps_first_n_then_suppresses() {
        let rule = Rule::CollapseRun {
            pattern: r"^ ".to_string(),
            keep_first: 2,
            ignore_blanks: false,
        };
        let mut compiled = rule.compile().unwrap();
        // First 2 matches: pass
        assert_eq!(compiled.apply(" line 1"), None);
        assert_eq!(compiled.apply(" line 2"), None);
        // 3rd match: suppress
        assert_eq!(
            compiled.apply(" line 3"),
            Some(FilterResult::SuppressWithNote(0))
        );
        // Non-match: reset
        assert_eq!(compiled.apply("+added"), None);
        // After reset, first match passes again
        assert_eq!(compiled.apply(" line 4"), None);
    }

    #[test]
    fn collapse_run_ignore_blanks_keeps_run_alive() {
        let rule = Rule::CollapseRun {
            pattern: r"^    ".to_string(),
            keep_first: 1,
            ignore_blanks: true,
        };
        let mut compiled = rule.compile().unwrap();
        assert_eq!(compiled.apply("    subject"), None); // first matches: keep
        assert_eq!(compiled.apply(""), None); // blank: passes, doesn't reset
        // Second matching line, count=2, suppressed
        assert_eq!(
            compiled.apply("    body"),
            Some(FilterResult::SuppressWithNote(0))
        );
    }

    #[test]
    fn truncate_match_no_op_when_no_match() {
        let rule = Rule::TruncateMatch {
            pattern: r"^XYZ".to_string(),
            replace: "abc".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        assert!(compiled.apply("plain text").is_none());
    }

    #[test]
    fn collapse_repeats_resets_on_unrelated_line() {
        let rule = Rule::CollapseRepeats {
            pattern: r"^Author: (.+)$".to_string(),
            match_capture: Some(1),
        };
        let mut compiled = rule.compile().unwrap();
        compiled.apply("Author: alice");
        // Unrelated line resets state
        compiled.apply("commit abc123");
        // Now alice again should NOT be suppressed (state was reset)
        assert_eq!(compiled.apply("Author: alice"), None);
    }
}
