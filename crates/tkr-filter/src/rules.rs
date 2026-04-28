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
}

pub enum RuleState {
    None,
    LastCapture(Option<String>),
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
