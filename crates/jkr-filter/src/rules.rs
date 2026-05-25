use indexmap::IndexMap;
use regex::{Captures, Regex, RegexBuilder, RegexSet};
use serde::Deserialize;
use std::collections::HashMap;
use jkr_api::FilterResult;

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
    /// Emits `message` at flush time if no line was ever observed by
    /// this rule's `apply`. Combined with the dispatch loop's
    /// short-circuit semantics, "observed" means "passed through every
    /// previous rule unsuppressed". MUST be the final rule in a group.
    EmptyResultSubstitute {
        message: String,
    },
    /// Suppress lines matching `pattern`; aggregate by capture group
    /// `key_capture` (default 1, falling back to group 0). At flush emit
    /// the FIRST line for each key, with `×N` suffix if N > 1.
    /// Insertion order preserved.
    DedupWithCount {
        pattern: String,
        #[serde(default)]
        key_capture: Option<usize>,
    },
    /// Bucket matching lines by `key_capture`. Keep at most
    /// `cap_per_key` lines per bucket and `total_cap` overall;
    /// excess increments per-bucket and global overflow counters.
    /// At flush emit a `header` followed by indented per-bucket lines.
    GroupByCapture {
        pattern: String,
        #[serde(default)]
        key_capture: Option<usize>,
        #[serde(default = "default_cap_per_key")]
        cap_per_key: u32,
        #[serde(default = "default_total_cap")]
        total_cap: u32,
        header: String,
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

fn default_cap_per_key() -> u32 {
    3
}

fn default_total_cap() -> u32 {
    50
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
    /// Coalesced run of consecutive `SuppressRegex` rules — matches any of
    /// the patterns in one DFA traversal. Inserted by `compile_group`; never
    /// constructed from a TOML `[[rules]]` entry directly. (KeepRegex is not
    /// similarly coalesced because chained KeepRegex is AND/intersection
    /// semantics, which RegexSet's OR/union behavior would break.)
    SuppressRegexSet {
        set: RegexSet,
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
    /// Coalesced run of consecutive `TruncateMatch` rules — one `RegexSet`
    /// precheck per line; only the first-indexed matching rule's replacement
    /// actually runs, matching the dispatch loop's first-match-wins behavior.
    /// Inserted by `compile_group`; never built from a TOML entry directly.
    TruncateMatchSet {
        precheck: RegexSet,
        rules: Vec<(Regex, String)>,
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
        ellipsis_chars: usize,
    },
    ContextWindow {
        re: Regex,
        around: usize,
    },
    EmptyResultSubstitute {
        message: String,
        observed_count: u64,
    },
    DedupWithCount {
        re: Regex,
        key_capture: Option<usize>,
        /// key -> (first_line_seen, count)
        seen: IndexMap<String, (String, u32)>,
    },
    GroupByCapture {
        re: Regex,
        key_capture: Option<usize>,
        cap_per_key: u32,
        total_cap: u32,
        header: String,
        /// key -> (kept_lines, overflow_count)
        buckets: IndexMap<String, (Vec<String>, u32)>,
        total_kept: u32,
        global_overflow: u32,
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
                let ellipsis_chars = ellipsis.chars().count();
                CompiledRule::TruncateLong {
                    max_len,
                    ellipsis,
                    ellipsis_chars,
                }
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
            Rule::EmptyResultSubstitute { message } => CompiledRule::EmptyResultSubstitute {
                message,
                observed_count: 0,
            },
            Rule::DedupWithCount {
                pattern,
                key_capture,
            } => CompiledRule::DedupWithCount {
                re: Regex::new(&pattern)?,
                key_capture,
                seen: IndexMap::new(),
            },
            Rule::GroupByCapture {
                pattern,
                key_capture,
                cap_per_key,
                total_cap,
                header,
            } => CompiledRule::GroupByCapture {
                re: Regex::new(&pattern)?,
                key_capture,
                cap_per_key,
                total_cap,
                header,
                buckets: IndexMap::new(),
                total_kept: 0,
                global_overflow: 0,
            },
        })
    }
}

/// Compile a group's rule list, coalescing consecutive `SuppressRegex` rules
/// into a single `SuppressRegexSet` and consecutive `TruncateMatch` rules
/// into a single `TruncateMatchSet`. Both convert per-line N-regex cost into
/// (roughly) one DFA traversal. Order is otherwise preserved.
pub fn compile_group(rules: Vec<Rule>) -> anyhow::Result<Vec<CompiledRule>> {
    let mut out = Vec::with_capacity(rules.len());
    let mut iter = rules.into_iter().peekable();
    while let Some(rule) = iter.next() {
        match rule {
            Rule::SuppressRegex { pattern } => {
                let mut patterns = vec![pattern];
                while matches!(iter.peek(), Some(Rule::SuppressRegex { .. })) {
                    if let Some(Rule::SuppressRegex { pattern }) = iter.next() {
                        patterns.push(pattern);
                    }
                }
                if patterns.len() == 1 {
                    out.push(CompiledRule::SuppressRegex {
                        re: Regex::new(&patterns[0])?,
                    });
                } else {
                    out.push(CompiledRule::SuppressRegexSet {
                        set: RegexSet::new(&patterns)?,
                    });
                }
            }
            Rule::TruncateMatch { pattern, replace } => {
                let mut pairs = vec![(pattern, replace)];
                while matches!(iter.peek(), Some(Rule::TruncateMatch { .. })) {
                    if let Some(Rule::TruncateMatch { pattern, replace }) = iter.next() {
                        pairs.push((pattern, replace));
                    }
                }
                if pairs.len() == 1 {
                    let (pattern, replace) = pairs.into_iter().next().unwrap();
                    out.push(CompiledRule::TruncateMatch {
                        re: Regex::new(&pattern)?,
                        replace,
                    });
                } else {
                    let patterns: Vec<&str> = pairs.iter().map(|(p, _)| p.as_str()).collect();
                    let precheck = RegexSet::new(&patterns)?;
                    let rules = pairs
                        .into_iter()
                        .map(|(p, r)| Ok::<_, anyhow::Error>((Regex::new(&p)?, r)))
                        .collect::<Result<Vec<_>, _>>()?;
                    out.push(CompiledRule::TruncateMatchSet { precheck, rules });
                }
            }
            other => out.push(other.compile()?),
        }
    }
    Ok(out)
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
            CompiledRule::SuppressRegexSet { set } => {
                if set.is_match(line) {
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
                // replace_all returns Cow::Borrowed when no substitution
                // happens, so checking the Cow variant is cheaper than the
                // byte-equality check the old code did.
                match re.replace_all(line, replace.as_str()) {
                    std::borrow::Cow::Borrowed(_) => None,
                    std::borrow::Cow::Owned(s) => Some(FilterResult::Replace(s)),
                }
            }
            CompiledRule::TruncateMatchSet { precheck, rules } => {
                let matches = precheck.matches(line);
                let Some(first_idx) = matches.iter().next() else {
                    return None;
                };
                let (re, replace) = &rules[first_idx];
                match re.replace_all(line, replace.as_str()) {
                    std::borrow::Cow::Borrowed(_) => None,
                    std::borrow::Cow::Owned(s) => Some(FilterResult::Replace(s)),
                }
            }
            CompiledRule::CollapseCommonPrefix {
                prefix_len,
                keep_first,
                last_prefix,
                count,
            } => {
                // Walk char_indices to find the byte cutoff for the first
                // `prefix_len` chars without allocating. Bail early if the
                // line has fewer chars than that — matches the original
                // `line.len() < prefix_len` reset semantics.
                let mut cutoff = 0;
                let mut chars_seen = 0;
                for (i, c) in line.char_indices() {
                    if chars_seen == *prefix_len {
                        cutoff = i;
                        break;
                    }
                    chars_seen += 1;
                    cutoff = i + c.len_utf8();
                }
                if chars_seen < *prefix_len {
                    *last_prefix = None;
                    *count = 0;
                    return None;
                }
                let cur = &line[..cutoff];
                let matches_prev = last_prefix.as_deref() == Some(cur);
                let new_count = if matches_prev { *count + 1 } else { 1 };
                // Only allocate when the prefix actually changes.
                if !matches_prev {
                    *last_prefix = Some(cur.to_string());
                }
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
            CompiledRule::TruncateLong {
                max_len,
                ellipsis,
                ellipsis_chars,
            } => {
                // Two byte-length shortcuts before any char walk:
                //   line.len() <= max_len    => char count <= max_len (skip)
                //   line.len() > max_len * 4 => char count > max_len (truncate)
                // (UTF-8 is 1-4 bytes/char, so byte length bounds char count.)
                // Only the middle band needs a bounded chars().nth() walk.
                if line.len() <= *max_len {
                    return None;
                }
                if line.len() <= max_len.saturating_mul(4)
                    && line.chars().nth(*max_len).is_none()
                {
                    return None;
                }
                let keep = max_len.saturating_sub(*ellipsis_chars);
                // Slice once at the char boundary instead of materializing a
                // Vec of chars and rebuilding a String.
                let cutoff = line
                    .char_indices()
                    .nth(keep)
                    .map(|(i, _)| i)
                    .unwrap_or(line.len());
                let mut out = String::with_capacity(cutoff + ellipsis.len());
                out.push_str(&line[..cutoff]);
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
                match new {
                    std::borrow::Cow::Borrowed(_) => None,
                    std::borrow::Cow::Owned(s) => Some(FilterResult::Replace(s)),
                }
            }
            CompiledRule::EmptyResultSubstitute {
                observed_count, ..
            } => {
                *observed_count += 1;
                None
            }
            CompiledRule::DedupWithCount {
                re,
                key_capture,
                seen,
            } => {
                let Some(caps) = re.captures(line) else {
                    return None;
                };
                let group_idx = key_capture.unwrap_or(1);
                let key_match = caps.get(group_idx).or_else(|| caps.get(0));
                let key_str = key_match.map(|m| m.as_str()).unwrap_or("");
                // Hit path: avoid allocating a new String when we already
                // have an entry for this key. IndexMap supports get_mut(&str)
                // even though entry() requires owned.
                if let Some(slot) = seen.get_mut(key_str) {
                    slot.1 += 1;
                } else {
                    seen.insert(key_str.to_string(), (line.to_string(), 1));
                }
                Some(FilterResult::Suppress)
            }
            CompiledRule::GroupByCapture {
                re,
                key_capture,
                cap_per_key,
                total_cap,
                buckets,
                total_kept,
                global_overflow,
                ..
            } => {
                let Some(caps) = re.captures(line) else {
                    return None;
                };
                let group_idx = key_capture.unwrap_or(1);
                let key_match = caps.get(group_idx).or_else(|| caps.get(0));
                let key_str = key_match.map(|m| m.as_str()).unwrap_or("");
                // Hit path uses get_mut(&str) to avoid the per-line key alloc.
                let bucket = if buckets.contains_key(key_str) {
                    buckets.get_mut(key_str).expect("just checked")
                } else {
                    buckets
                        .entry(key_str.to_string())
                        .or_insert_with(|| (Vec::new(), 0))
                };
                let under_per_key = (bucket.0.len() as u32) < *cap_per_key;
                let under_total = *total_kept < *total_cap;
                if under_per_key && under_total {
                    bucket.0.push(line.to_string());
                    *total_kept += 1;
                } else if !under_per_key && under_total {
                    bucket.1 += 1;
                } else {
                    *global_overflow += 1;
                }
                Some(FilterResult::Suppress)
            }
        }
    }

    /// Emit any end-of-command summary text. Default: none. Aggregating
    /// rules (DedupWithCount, GroupByCapture, EmptyResultSubstitute) override.
    pub fn flush_summary(&mut self) -> Option<String> {
        match self {
            CompiledRule::EmptyResultSubstitute {
                message,
                observed_count,
            } => {
                if *observed_count == 0 {
                    Some(message.clone())
                } else {
                    None
                }
            }
            CompiledRule::DedupWithCount { seen, .. } => {
                if seen.is_empty() {
                    return None;
                }
                let mut out = String::new();
                for (i, (_, (line, count))) in seen.iter().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    if *count > 1 {
                        out.push_str(&format!("{line} ×{count}"));
                    } else {
                        out.push_str(line);
                    }
                }
                Some(out)
            }
            CompiledRule::GroupByCapture {
                header,
                buckets,
                global_overflow,
                ..
            } => {
                if buckets.is_empty() {
                    return None;
                }
                let mut out = String::new();
                out.push_str(header);
                for (key, (lines, overflow)) in buckets.iter() {
                    out.push_str("\n  ");
                    out.push_str(key);
                    out.push_str(": ");
                    out.push_str(&lines.join(", "));
                    if *overflow > 0 {
                        out.push_str(&format!(" (+{overflow})"));
                    }
                }
                if *global_overflow > 0 {
                    out.push_str(&format!("\n  (+{global_overflow} dropped)"));
                }
                Some(out)
            }
            _ => None,
        }
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
    fn compile_group_coalesces_consecutive_suppress_regex() {
        let rules = vec![
            Rule::SuppressRegex {
                pattern: "^foo".into(),
            },
            Rule::SuppressRegex {
                pattern: "^bar".into(),
            },
            Rule::SuppressRegex {
                pattern: "^baz".into(),
            },
        ];
        let compiled = compile_group(rules).expect("compile");
        assert_eq!(compiled.len(), 1);
        assert!(matches!(&compiled[0], CompiledRule::SuppressRegexSet { .. }));
        let mut compiled = compiled;
        assert_eq!(compiled[0].apply("foo line"), Some(FilterResult::Suppress));
        assert_eq!(compiled[0].apply("bar line"), Some(FilterResult::Suppress));
        assert_eq!(compiled[0].apply("baz line"), Some(FilterResult::Suppress));
        assert_eq!(compiled[0].apply("other line"), None);
    }

    #[test]
    fn compile_group_does_not_coalesce_across_other_rules() {
        let rules = vec![
            Rule::SuppressRegex {
                pattern: "^foo".into(),
            },
            Rule::TruncateMatch {
                pattern: "x".into(),
                replace: "y".into(),
            },
            Rule::SuppressRegex {
                pattern: "^bar".into(),
            },
        ];
        let compiled = compile_group(rules).expect("compile");
        // Order preserved: suppress, truncate, suppress — no coalescing across
        // the TruncateMatch barrier.
        assert_eq!(compiled.len(), 3);
        assert!(matches!(&compiled[0], CompiledRule::SuppressRegex { .. }));
        assert!(matches!(&compiled[1], CompiledRule::TruncateMatch { .. }));
        assert!(matches!(&compiled[2], CompiledRule::SuppressRegex { .. }));
    }

    #[test]
    fn compile_group_coalesces_consecutive_truncate_match() {
        let rules = vec![
            Rule::TruncateMatch {
                pattern: r"\b[a-f0-9]{40}\b".into(),
                replace: "<sha>".into(),
            },
            Rule::TruncateMatch {
                pattern: r"/home/[^/]+/".into(),
                replace: "~/".into(),
            },
        ];
        let compiled = compile_group(rules).expect("compile");
        assert_eq!(compiled.len(), 1);
        assert!(matches!(&compiled[0], CompiledRule::TruncateMatchSet { .. }));
        let mut compiled = compiled;
        // First-match-wins semantics: only one replacement applies per line.
        match compiled[0].apply("abcdef0123456789abcdef0123456789abcdef01") {
            Some(FilterResult::Replace(s)) => assert!(s.contains("<sha>")),
            other => panic!("expected Replace, got {other:?}"),
        }
        // Line that matches neither pattern: None.
        assert_eq!(compiled[0].apply("nothing interesting here"), None);
    }

    #[test]
    fn compile_group_keeps_single_suppress_regex_uncoalesced() {
        let rules = vec![Rule::SuppressRegex {
            pattern: "^foo".into(),
        }];
        let compiled = compile_group(rules).expect("compile");
        assert_eq!(compiled.len(), 1);
        assert!(matches!(&compiled[0], CompiledRule::SuppressRegex { .. }));
    }

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

    #[test]
    fn empty_result_substitute_emits_message_when_no_lines_pass() {
        let rule = Rule::EmptyResultSubstitute {
            message: "0 matches".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        assert_eq!(compiled.flush_summary(), Some("0 matches".to_string()));
    }

    #[test]
    fn empty_result_substitute_silent_when_lines_passed() {
        let rule = Rule::EmptyResultSubstitute {
            message: "0 matches".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        assert_eq!(compiled.apply("a real result line"), None);
        assert_eq!(compiled.flush_summary(), None);
    }

    #[test]
    fn dedup_with_count_aggregates_repeats() {
        let rule = Rule::DedupWithCount {
            pattern: r"^Error: (\w+)".to_string(),
            key_capture: Some(1),
        };
        let mut compiled = rule.compile().unwrap();
        assert_eq!(compiled.apply("Error: E001 happened"), Some(FilterResult::Suppress));
        assert_eq!(compiled.apply("Error: E001 again"), Some(FilterResult::Suppress));
        assert_eq!(compiled.apply("Error: E002 different"), Some(FilterResult::Suppress));
        assert_eq!(compiled.apply("Error: E001 third time"), Some(FilterResult::Suppress));
        assert_eq!(compiled.apply("not an error line"), None);

        let summary = compiled.flush_summary().expect("summary");
        assert!(summary.contains("Error: E001 happened ×3"), "got {summary:?}");
        assert!(summary.contains("Error: E002 different"), "got {summary:?}");
        assert!(!summary.contains("Error: E002 different ×"), "got {summary:?}");
        let p1 = summary.find("E001").unwrap();
        let p2 = summary.find("E002").unwrap();
        assert!(p1 < p2);
    }

    #[test]
    fn dedup_with_count_no_match_passes_through() {
        let rule = Rule::DedupWithCount {
            pattern: r"^Error: (\w+)".to_string(),
            key_capture: Some(1),
        };
        let mut compiled = rule.compile().unwrap();
        assert_eq!(compiled.apply("plain line"), None);
        assert_eq!(compiled.flush_summary(), None);
    }

    #[test]
    fn group_by_capture_buckets_and_caps() {
        let rule = Rule::GroupByCapture {
            pattern: r"^([^:]+):".to_string(),
            key_capture: Some(1),
            cap_per_key: 2,
            total_cap: 10,
            header: "Matches by file:".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        assert_eq!(compiled.apply("fileA: hit 1"), Some(FilterResult::Suppress));
        assert_eq!(compiled.apply("fileA: hit 2"), Some(FilterResult::Suppress));
        assert_eq!(compiled.apply("fileA: hit 3"), Some(FilterResult::Suppress));
        assert_eq!(compiled.apply("fileB: hit 1"), Some(FilterResult::Suppress));
        assert_eq!(compiled.apply("no colon line"), None);

        let summary = compiled.flush_summary().expect("summary");
        assert!(summary.starts_with("Matches by file:"), "got {summary:?}");
        assert!(summary.contains("fileA"), "got {summary:?}");
        assert!(summary.contains("hit 1"), "got {summary:?}");
        assert!(summary.contains("hit 2"), "got {summary:?}");
        assert!(summary.contains("(+1)"), "got {summary:?}");
        assert!(summary.contains("fileB"), "got {summary:?}");
    }

    #[test]
    fn group_by_capture_total_cap_caps_globally() {
        let rule = Rule::GroupByCapture {
            pattern: r"^(\w+):".to_string(),
            key_capture: Some(1),
            cap_per_key: 5,
            total_cap: 2,
            header: "Hits:".to_string(),
        };
        let mut compiled = rule.compile().unwrap();
        compiled.apply("a: 1");
        compiled.apply("b: 1");
        compiled.apply("c: 1"); // exceeds total_cap
        let summary = compiled.flush_summary().expect("summary");
        assert!(summary.contains("(+1 dropped)"), "got {summary:?}");
    }
}
