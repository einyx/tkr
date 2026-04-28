//! Phase-1 noise ranking: group emitted lines by shape signature and surface
//! the heaviest-traffic groups as `suppress_regex` candidates. Phases 2 and 3
//! layer RRF combination and embedding-based ranking on top of this.

use std::collections::HashMap;

use crate::signature::signature_of;

pub struct KeptLine<'a> {
    pub command: &'a str,
    pub line: &'a str,
}

#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub command: String,
    pub signature: String,
    pub sample: String,
    pub occurrences: u64,
    pub total_chars: u64,
    pub source: &'static str,
}

pub trait NoiseRanker {
    fn rank(&mut self, lines: &[KeptLine]) -> Vec<RankedCandidate>;
}

pub struct ShapeRanker;

impl NoiseRanker for ShapeRanker {
    fn rank(&mut self, lines: &[KeptLine]) -> Vec<RankedCandidate> {
        let mut groups: HashMap<(String, String), RankedCandidate> = HashMap::new();
        for kl in lines {
            let sig = signature_of(kl.line);
            let key = (kl.command.to_string(), sig.clone());
            let entry = groups.entry(key).or_insert_with(|| RankedCandidate {
                command: kl.command.to_string(),
                signature: sig,
                sample: kl.line.to_string(),
                occurrences: 0,
                total_chars: 0,
                source: "shape",
            });
            entry.occurrences += 1;
            entry.total_chars += kl.line.len() as u64;
        }
        let mut out: Vec<RankedCandidate> = groups
            .into_values()
            .filter(|c| c.occurrences >= 2)
            .collect();
        out.sort_by(|a, b| b.total_chars.cmp(&a.total_chars));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_same_shape_lines() {
        let lines = vec![
            KeptLine { command: "git", line: "[2026-04-28T10:00:00] tick" },
            KeptLine { command: "git", line: "[2026-04-28T10:00:01] tick" },
            KeptLine { command: "git", line: "[2026-04-28T10:00:02] tick" },
        ];
        let mut r = ShapeRanker;
        let ranked = r.rank(&lines);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].occurrences, 3);
        assert_eq!(ranked[0].source, "shape");
    }

    #[test]
    fn singletons_are_dropped() {
        let lines = vec![
            KeptLine { command: "git", line: "unique line one" },
            KeptLine { command: "git", line: "totally different content here" },
        ];
        let mut r = ShapeRanker;
        assert!(r.rank(&lines).is_empty());
    }

    #[test]
    fn sorts_by_total_chars_desc() {
        let lines = vec![
            KeptLine { command: "git", line: "short 1" },
            KeptLine { command: "git", line: "short 2" },
            KeptLine { command: "git", line: "a much longer line with number 1 here" },
            KeptLine { command: "git", line: "a much longer line with number 2 here" },
        ];
        let mut r = ShapeRanker;
        let ranked = r.rank(&lines);
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].total_chars > ranked[1].total_chars);
    }

    #[test]
    fn separates_by_command() {
        let lines = vec![
            KeptLine { command: "git", line: "shared shape 1" },
            KeptLine { command: "git", line: "shared shape 2" },
            KeptLine { command: "cargo", line: "shared shape 1" },
            KeptLine { command: "cargo", line: "shared shape 2" },
        ];
        let mut r = ShapeRanker;
        let ranked = r.rank(&lines);
        assert_eq!(ranked.len(), 2);
    }
}
