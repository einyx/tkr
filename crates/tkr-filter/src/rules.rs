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
}

fn default_prefix_len() -> usize {
    24
}

fn default_keep_first() -> u32 {
    3
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
}
