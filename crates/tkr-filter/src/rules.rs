use regex::{Captures, Regex, RegexBuilder};
use serde::Deserialize;
use std::collections::HashMap;
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
    /// Suppress consecutive lines that match `pattern` and produce the same
    /// captured group value. `match_capture` defaults to group 1.
    CollapseRepeats {
        pattern: String,
        #[serde(default)]
        match_capture: Option<usize>,
    },
    /// Replace each match of `pattern` with `replace` (regex replacement
    /// template — supports `$1`, `$name`).
    TruncateMatch {
        pattern: String,
        replace: String,
    },
    /// Suppress consecutive matches of `pattern` past the first `keep_first`.
    /// `ignore_blanks`: blank lines neither match nor reset the run.
    CollapseRun {
        pattern: String,
        #[serde(default = "default_keep_first")]
        keep_first: u32,
        #[serde(default)]
        ignore_blanks: bool,
    },
    /// Suppress consecutive lines sharing the same first `prefix_len` chars
    /// past the first `keep_first`. Catches log lines with timestamp prefixes.
    CollapseCommonPrefix {
        #[serde(default = "default_prefix_len")]
        prefix_len: usize,
        #[serde(default = "default_keep_first")]
        keep_first: u32,
    },
    /// Replace lines longer than `max_len` chars with their first
    /// (max_len - ellipsis_chars) chars followed by `ellipsis`.
    /// Operates on Unicode chars, not bytes.
    TruncateLong {
        max_len: usize,
        #[serde(default = "default_ellipsis")]
        ellipsis: String,
    },
    /// Replace the line with `around` chars before + match + `around` chars
    /// after the FIRST match of `pattern`. If no match, line passes
    /// through unchanged. Char-based, not byte-based.
    ContextWindow {
        pattern: String,
        around: usize,
    },
    /// Substitute words in the line according to a `pairs` dictionary.
    /// Each pair is `[word, abbreviation]`. Match is case-insensitive
    /// at word boundaries; the original case is preserved (lowercase
    /// stays lowercase, Capitalized stays Capitalized, ALL-CAPS stays
    /// ALL-CAPS). The whole line passes through with replacements
    /// applied; this rule never suppresses or short-circuits the
    /// pipeline (returns `None` if the line was unchanged after substitution).
    SubstituteWords {
        pairs: Vec<(String, String)>,
    },
}

fn default_prefix_len() -> usize {
    24
}

fn default_keep_first() -> u32 {
    3
}

fn default_ellipsis() -> String {
    "…".to_string()
}

/// Compiled rule with state inlined per variant. `apply` takes &mut self so
/// stateful rules (CollapseRepeats / Run / CommonPrefix) can update directly.
pub enum CompiledRule {
    SuppressPrefix {
        prefix: String,
    },
    SuppressRegex {
        re: Regex,
    },
    KeepRegex {
        re: Regex,
    },
    CollapseRepeats {
        re: Regex,
        group: Option<usize>,
        last_capture: Option<String>,
    },
    TruncateMatch {
        re: Regex,
        replace: String,
    },
    CollapseRun {
        re: Regex,
        keep_first: u32,
        ignore_blanks: bool,
        count: u32,
    },
    CollapseCommonPrefix {
        prefix_len: usize,
        keep_first: u32,
        last_prefix: Option<String>,
        count: u32,
    },
    SubstituteWords {
        re: Regex,
        dict: HashMap<String, String>,
    },
    TruncateLong {
        max_len: usize,
        ellipsis: String,
    },
    ContextWindow {
        re: Regex,
        around: usize,
    },
}

impl Rule {
    pub fn compile(self) -> anyhow::Result<CompiledRule> {
        Ok(match self {
            Rule::SuppressPrefix { prefix } => CompiledRule::SuppressPrefix { prefix },
            Rule::SuppressRegex { pattern } => CompiledRule::SuppressRegex {
                re: Regex::new(&pattern)?,
            },
            Rule::KeepRegex { pattern } => CompiledRule::KeepRegex {
                re: Regex::new(&pattern)?,
            },
            Rule::CollapseRepeats {
                pattern,
                match_capture,
            } => CompiledRule::CollapseRepeats {
                re: Regex::new(&pattern)?,
                group: match_capture,
                last_capture: None,
            },
            Rule::TruncateMatch { pattern, replace } => CompiledRule::TruncateMatch {
                re: Regex::new(&pattern)?,
                replace,
            },
            Rule::CollapseRun {
                pattern,
                keep_first,
                ignore_blanks,
            } => CompiledRule::CollapseRun {
                re: Regex::new(&pattern)?,
                keep_first,
                ignore_blanks,
                count: 0,
            },
            Rule::CollapseCommonPrefix {
                prefix_len,
                keep_first,
            } => CompiledRule::CollapseCommonPrefix {
                prefix_len,
                keep_first,
                last_prefix: None,
                count: 0,
            },
            Rule::TruncateLong { max_len, ellipsis } => {
                CompiledRule::TruncateLong { max_len, ellipsis }
            }
            Rule::ContextWindow { pattern, around } => CompiledRule::ContextWindow {
                re: Regex::new(&pattern)?,
                around,
            },
            Rule::SubstituteWords { pairs } => {
                if pairs.is_empty() {
                    anyhow::bail!("substitute_words: pairs cannot be empty");
                }
                // Build a single regex with alternation: \b(word1|word2|...)\b.
                // Words are sorted longest-first so multi-word forms (if any)
                // win over their prefixes. Each word is regex-escaped.
                let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
                sorted.sort_by_key(|(w, _)| std::cmp::Reverse(w.len()));
                let alternation = sorted
                    .iter()
                    .map(|(w, _)| regex::escape(w))
                    .collect::<Vec<_>>()
                    .join("|");
                let pattern = format!(r"\b(?:{alternation})\b");
                let re = RegexBuilder::new(&pattern).case_insensitive(true).build()?;
                let dict: HashMap<String, String> = pairs
                    .into_iter()
                    .map(|(w, a)| (w.to_lowercase(), a))
                    .collect();
                CompiledRule::SubstituteWords { re, dict }
            }
        })
    }
}

impl CompiledRule {
    /// Returns `Some(FilterResult)` if this rule has an opinion, `None` to defer.
    pub fn apply(&mut self, line: &str) -> Option<FilterResult> {
        match self {
            CompiledRule::SuppressPrefix { prefix } => {
                if line.starts_with(prefix.as_str()) {
                    Some(FilterResult::Suppress)
                } else {
                    None
                }
            }
            CompiledRule::SuppressRegex { re } => {
                if re.is_match(line) {
                    Some(FilterResult::Suppress)
                } else {
                    None
                }
            }
            CompiledRule::KeepRegex { re } => {
                if re.is_match(line) {
                    None
                } else {
                    Some(FilterResult::Suppress)
                }
            }
            CompiledRule::TruncateMatch { re, replace } => {
                let new = re.replace_all(line, replace.as_str());
                if new == line {
                    None
                } else {
                    Some(FilterResult::Replace(new.into_owned()))
                }
            }
            CompiledRule::CollapseCommonPrefix {
                prefix_len,
                keep_first,
                last_prefix,
                count,
            } => {
                if line.len() < *prefix_len {
                    *last_prefix = None;
                    *count = 0;
                    return None;
                }
                let cur: String = line.chars().take(*prefix_len).collect();
                let new_count = match last_prefix.as_ref() {
                    Some(p) if p == &cur => *count + 1,
                    _ => 1,
                };
                *last_prefix = Some(cur);
                *count = new_count;
                if new_count > *keep_first {
                    Some(FilterResult::SuppressWithNote(0))
                } else {
                    None
                }
            }
            CompiledRule::CollapseRun {
                re,
                keep_first,
                ignore_blanks,
                count,
            } => {
                if re.is_match(line) {
                    *count += 1;
                    if *count > *keep_first {
                        Some(FilterResult::SuppressWithNote(0))
                    } else {
                        None
                    }
                } else if *ignore_blanks && line.trim().is_empty() {
                    None
                } else {
                    *count = 0;
                    None
                }
            }
            CompiledRule::CollapseRepeats {
                re,
                group,
                last_capture,
            } => {
                if let Some(caps) = re.captures(line) {
                    let group_idx = group.unwrap_or(1);
                    let current = caps
                        .get(group_idx)
                        .or_else(|| caps.get(0))
                        .map(|m| m.as_str().to_string());
                    let suppress = matches!(
                        (last_capture.as_ref(), current.as_ref()),
                        (Some(prev), Some(now)) if prev == now
                    );
                    *last_capture = current;
                    if suppress {
                        Some(FilterResult::Suppress)
                    } else {
                        None
                    }
                } else {
                    *last_capture = None;
                    None
                }
            }
            CompiledRule::TruncateLong { max_len, ellipsis } => {
                let char_count = line.chars().count();
                if char_count <= *max_len {
                    return None;
                }
                let ellipsis_chars = ellipsis.chars().count();
                let keep = max_len.saturating_sub(ellipsis_chars);
                let mut out: String = line.chars().take(keep).collect();
                out.push_str(ellipsis);
                Some(FilterResult::Replace(out))
            }
            CompiledRule::ContextWindow { re, around } => {
                let Some(m) = re.find(line) else {
                    return None;
                };
                let pre_str = &line[..m.start()];
                let post_str = &line[m.end()..];
                let pre_chars: Vec<char> = pre_str.chars().collect();
                let post_chars: Vec<char> = post_str.chars().collect();
                let pre_cut = pre_chars.len().saturating_sub(*around);
                let post_keep = (*around).min(post_chars.len());
                let mut out = String::new();
                if pre_cut > 0 {
                    out.push('…');
                }
                out.extend(pre_chars.iter().skip(pre_cut));
                out.push_str(m.as_str());
                out.extend(post_chars.iter().take(post_keep));
                if post_keep < post_chars.len() {
                    out.push('…');
                }
                Some(FilterResult::Replace(out))
            }
            CompiledRule::SubstituteWords { re, dict } => {
                let new = re.replace_all(line, |caps: &Captures| {
                    let matched = &caps[0];
                    match dict.get(&matched.to_lowercase()) {
                        Some(abbrev) => preserve_case(matched, abbrev),
                        None => matched.to_string(),
                    }
                });
                if new == line {
                    None
                } else {
                    Some(FilterResult::Replace(new.into_owned()))
                }
            }
        }
    }

    /// Emit any end-of-command summary text. Default: none. Aggregating
    /// rules (DedupWithCount, GroupByCapture, EmptyResultSubstitute) override.
    pub fn flush_summary(&mut self) -> Option<String> {
        let _ = self;
        None
    }
}

/// Replicate the case shape of `original` onto `abbrev`. Three cases:
/// - ALL-UPPERCASE original (≥ 2 chars) → uppercase abbrev
/// - Capitalized (first upper, rest lower) → capitalize abbrev
/// - everything else → abbrev unchanged
fn preserve_case(original: &str, abbrev: &str) -> String {
    let alpha_chars: Vec<char> = original.chars().filter(|c| c.is_alphabetic()).collect();
    if alpha_chars.len() >= 2 && alpha_chars.iter().all(|c| c.is_uppercase()) {
        return abbrev.to_uppercase();
    }
    let mut chars = original.chars();
    if let Some(first) = chars.next() {
        let tail_has_upper = chars.any(|c| c.is_uppercase());
        if first.is_uppercase() && !tail_has_upper {
            let mut out = String::with_capacity(abbrev.len());
            let mut ac = abbrev.chars();
            if let Some(a0) = ac.next() {
                for u in a0.to_uppercase() {
                    out.push(u);
                }
            }
            out.extend(ac);
            return out;
        }
    }
    abbrev.to_string()
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
        assert_eq!(compiled.apply("Author: alice"), None);
        assert_eq!(
            compiled.apply("Author: alice"),
            Some(FilterResult::Suppress)
        );
        assert_eq!(compiled.apply("Author: bob"), None);
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
            Some(FilterResult::Replace(s)) => assert_eq!(s, "commit 81cda43"),
            other => panic!("expected Replace, got {other:?}"),
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
        assert_eq!(compiled.apply(" line 1"), None);
        assert_eq!(compiled.apply(" line 2"), None);
        assert_eq!(
            compiled.apply(" line 3"),
            Some(FilterResult::SuppressWithNote(0))
        );
        assert_eq!(compiled.apply("+added"), None);
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
        assert_eq!(compiled.apply("    subject"), None);
        assert_eq!(compiled.apply(""), None);
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
        compiled.apply("commit abc123");
        assert_eq!(compiled.apply("Author: alice"), None);
    }

    #[test]
    fn truncate_long_caps_oversize_lines() {
        let rule = Rule::TruncateLong {
            max_len: 10,
            ellipsis: "…".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        let result = compiled.apply("abcdefghijklmnop");
        match result {
            Some(FilterResult::Replace(s)) => {
                assert!(s.chars().count() <= 10, "got {s:?}");
                assert!(s.ends_with('…'), "got {s:?}");
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn truncate_long_passes_short_lines() {
        let rule = Rule::TruncateLong {
            max_len: 10,
            ellipsis: "…".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        assert_eq!(compiled.apply("short"), None);
    }

    #[test]
    fn truncate_long_handles_multibyte_boundary() {
        let rule = Rule::TruncateLong {
            max_len: 5,
            ellipsis: "…".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        let result = compiled.apply("héllo wörld");
        assert!(matches!(result, Some(FilterResult::Replace(_))));
    }

    #[test]
    fn context_window_slices_around_match() {
        let rule = Rule::ContextWindow {
            pattern: r"NEEDLE".to_string(),
            around: 5,
        };
        let mut compiled = rule.compile().unwrap();
        let line = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxNEEDLEyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy";
        let result = compiled.apply(line);
        match result {
            Some(FilterResult::Replace(s)) => {
                assert!(s.contains("NEEDLE"), "got {s:?}");
                assert!(s.chars().count() <= "NEEDLE".len() + 10 + 6, "got {s:?}");
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn context_window_no_op_on_no_match() {
        let rule = Rule::ContextWindow {
            pattern: r"NEEDLE".to_string(),
            around: 5,
        };
        let mut compiled = rule.compile().unwrap();
        assert_eq!(compiled.apply("nothing here"), None);
    }

    #[test]
    fn context_window_keeps_short_lines_unchanged_only_if_match_centered() {
        let rule = Rule::ContextWindow {
            pattern: r"hi".to_string(),
            around: 100,
        };
        let mut compiled = rule.compile().unwrap();
        let result = compiled.apply("say hi there");
        match result {
            Some(FilterResult::Replace(s)) => assert!(s.contains("hi")),
            None => {} // also acceptable
            other => panic!("unexpected {other:?}"),
        }
    }
}
