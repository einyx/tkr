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

/// Reciprocal Rank Fusion: combine rankings from multiple `NoiseRanker`s into
/// a single ranked list. Robust to scale differences between ranker scores —
/// only positions matter. Standard formula: `score(c) = Σ 1/(k + rank_i(c))`.
///
/// Candidates are merged by `(command, signature)`. When the same key appears
/// in multiple sources, occurrences/total_chars are taken from the source with
/// the larger count, and `source` is set to "rrf".
pub struct RrfCombiner {
    pub k: f64,
}

impl Default for RrfCombiner {
    fn default() -> Self {
        // k=60 is the canonical value from the original RRF paper.
        Self { k: 60.0 }
    }
}

impl RrfCombiner {
    pub fn fuse(&self, rankings: Vec<Vec<RankedCandidate>>) -> Vec<RankedCandidate> {
        let mut scored: HashMap<(String, String), (RankedCandidate, f64)> = HashMap::new();

        for ranking in &rankings {
            for (rank, c) in ranking.iter().enumerate() {
                let key = (c.command.clone(), c.signature.clone());
                let contribution = 1.0 / (self.k + rank as f64 + 1.0);
                let entry = scored.entry(key).or_insert_with(|| (c.clone(), 0.0));
                // Merge: keep the candidate with the larger occurrence count.
                if c.occurrences > entry.0.occurrences {
                    let prev_score = entry.1;
                    entry.0 = c.clone();
                    entry.1 = prev_score;
                }
                entry.1 += contribution;
            }
        }

        let mut fused: Vec<(RankedCandidate, f64)> = scored.into_values().collect();
        fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        fused
            .into_iter()
            .map(|(mut c, _)| {
                c.source = "rrf";
                c
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(cmd: &str, sig: &str, occ: u64, chars: u64) -> RankedCandidate {
        RankedCandidate {
            command: cmd.into(),
            signature: sig.into(),
            sample: format!("{sig} sample"),
            occurrences: occ,
            total_chars: chars,
            source: "test",
        }
    }

    #[test]
    fn rrf_merges_overlapping_candidates() {
        // alpha appears at rank 0 in BOTH rankings (unambiguous winner).
        // beta appears at rank 1 in both. gamma only appears in b.
        let a = vec![cand("git", "alpha", 5, 500), cand("git", "beta", 3, 300)];
        let b = vec![
            cand("git", "alpha", 2, 200),
            cand("git", "beta", 4, 400),
            cand("git", "gamma", 6, 600),
        ];
        let fused = RrfCombiner::default().fuse(vec![a, b]);
        assert!(fused.iter().any(|c| c.signature == "alpha"));
        assert!(fused.iter().any(|c| c.signature == "beta"));
        assert!(fused.iter().any(|c| c.signature == "gamma"));
        assert_eq!(fused[0].signature, "alpha"); // best position in both
        assert_eq!(fused[1].signature, "beta"); // second in both
        assert_eq!(fused[2].signature, "gamma"); // only in one ranker
    }

    #[test]
    fn rrf_picks_higher_occurrence_data() {
        let a = vec![cand("git", "x", 5, 500)];
        let b = vec![cand("git", "x", 10, 1000)];
        let fused = RrfCombiner::default().fuse(vec![a, b]);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].occurrences, 10);
        assert_eq!(fused[0].total_chars, 1000);
        assert_eq!(fused[0].source, "rrf");
    }

    #[test]
    fn rrf_empty_inputs_ok() {
        let fused = RrfCombiner::default().fuse(vec![]);
        assert!(fused.is_empty());
    }

    #[test]
    fn rrf_single_ranker_passthrough_order() {
        let single = vec![
            cand("git", "a", 5, 500),
            cand("git", "b", 3, 300),
            cand("git", "c", 2, 200),
        ];
        let fused = RrfCombiner::default().fuse(vec![single]);
        assert_eq!(fused[0].signature, "a");
        assert_eq!(fused[1].signature, "b");
        assert_eq!(fused[2].signature, "c");
    }

    #[test]
    fn groups_same_shape_lines() {
        let lines = vec![
            KeptLine {
                command: "git",
                line: "[2026-04-28T10:00:00] tick",
            },
            KeptLine {
                command: "git",
                line: "[2026-04-28T10:00:01] tick",
            },
            KeptLine {
                command: "git",
                line: "[2026-04-28T10:00:02] tick",
            },
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
            KeptLine {
                command: "git",
                line: "unique line one",
            },
            KeptLine {
                command: "git",
                line: "totally different content here",
            },
        ];
        let mut r = ShapeRanker;
        assert!(r.rank(&lines).is_empty());
    }

    #[test]
    fn sorts_by_total_chars_desc() {
        let lines = vec![
            KeptLine {
                command: "git",
                line: "short 1",
            },
            KeptLine {
                command: "git",
                line: "short 2",
            },
            KeptLine {
                command: "git",
                line: "a much longer line with number 1 here",
            },
            KeptLine {
                command: "git",
                line: "a much longer line with number 2 here",
            },
        ];
        let mut r = ShapeRanker;
        let ranked = r.rank(&lines);
        assert_eq!(ranked.len(), 2);
        assert!(ranked[0].total_chars > ranked[1].total_chars);
    }

    #[test]
    fn separates_by_command() {
        let lines = vec![
            KeptLine {
                command: "git",
                line: "shared shape 1",
            },
            KeptLine {
                command: "git",
                line: "shared shape 2",
            },
            KeptLine {
                command: "cargo",
                line: "shared shape 1",
            },
            KeptLine {
                command: "cargo",
                line: "shared shape 2",
            },
        ];
        let mut r = ShapeRanker;
        let ranked = r.rank(&lines);
        assert_eq!(ranked.len(), 2);
    }
}
