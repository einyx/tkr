//! Phase-3 noise ranking: cluster emitted lines by embedding cosine similarity.
//! Catches near-duplicates that the shape ranker misses (lines with the same
//! meaning but different tokens — e.g. "user X logged in" vs "X authenticated").
//!
//! Feature-gated behind `embeddings`. Without the feature, this module exposes
//! a stub `EmbeddingRanker` that returns no candidates — `RrfCombiner` then
//! degenerates gracefully to shape-only ranking.

#[cfg(feature = "embeddings")]
use crate::noise_ranker::RankedCandidate;
use crate::noise_ranker::{KeptLine, NoiseRanker};

#[cfg(not(feature = "embeddings"))]
pub struct EmbeddingRanker;

#[cfg(not(feature = "embeddings"))]
impl EmbeddingRanker {
    pub fn new() -> Option<Self> {
        None
    }
}

#[cfg(not(feature = "embeddings"))]
impl Default for EmbeddingRanker {
    fn default() -> Self {
        Self
    }
}

#[cfg(not(feature = "embeddings"))]
impl NoiseRanker for EmbeddingRanker {
    fn rank(&mut self, _lines: &[KeptLine]) -> Vec<crate::noise_ranker::RankedCandidate> {
        Vec::new()
    }
}

#[cfg(feature = "embeddings")]
mod imp {
    use super::*;
    use crate::signature::signature_of;
    use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

    /// Cosine similarity of L2-normalized vectors. fastembed yields normalized
    /// embeddings already, so we just dot-product.
    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    pub struct EmbeddingRanker {
        model: TextEmbedding,
        threshold: f32,
        max_lines: usize,
    }

    impl EmbeddingRanker {
        pub fn new() -> Option<Self> {
            // AllMiniLML6V2 — 384 dim, ~80MB, downloads to ~/.cache/fastembed.
            let model = TextEmbedding::try_new(InitOptions::new(EmbeddingModel::AllMiniLML6V2))
                .ok()?;
            Some(Self {
                model,
                threshold: 0.85,
                max_lines: 5000,
            })
        }
    }

    impl NoiseRanker for EmbeddingRanker {
        fn rank(&mut self, lines: &[KeptLine]) -> Vec<RankedCandidate> {
            if lines.len() < 4 {
                return Vec::new();
            }
            // Cap input — embedding all of a 50K-line dump is too slow for
            // an interactive `tkr suggest` invocation.
            let take = lines.len().min(self.max_lines);
            let texts: Vec<&str> = lines.iter().take(take).map(|l| l.line).collect();
            let embeddings = match self.model.embed(texts.clone(), None) {
                Ok(e) => e,
                Err(_) => return Vec::new(),
            };

            // Single-link clustering: assign each line to the first existing
            // cluster whose centroid is within `threshold` cosine similarity.
            let mut centroids: Vec<Vec<f32>> = Vec::new();
            let mut clusters: Vec<Vec<usize>> = Vec::new();
            for (i, emb) in embeddings.iter().enumerate() {
                let mut placed = false;
                for (cidx, centroid) in centroids.iter().enumerate() {
                    if cosine(emb, centroid) >= self.threshold {
                        clusters[cidx].push(i);
                        placed = true;
                        break;
                    }
                }
                if !placed {
                    centroids.push(emb.clone());
                    clusters.push(vec![i]);
                }
            }

            // For each cluster with ≥ 3 members, emit a candidate using the
            // shape-signature of the first member as the key (so RRF can merge
            // with the shape ranker on overlap).
            let mut out: Vec<RankedCandidate> = clusters
                .into_iter()
                .filter(|c| c.len() >= 3)
                .map(|members| {
                    let head = &lines[members[0]];
                    let occurrences = members.len() as u64;
                    let total_chars: u64 =
                        members.iter().map(|&i| lines[i].line.len() as u64).sum();
                    RankedCandidate {
                        command: head.command.to_string(),
                        signature: signature_of(head.line),
                        sample: head.line.to_string(),
                        occurrences,
                        total_chars,
                        source: "embedding",
                    }
                })
                .collect();
            out.sort_by(|a, b| b.total_chars.cmp(&a.total_chars));
            out
        }
    }
}

#[cfg(feature = "embeddings")]
pub use imp::EmbeddingRanker;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranker_compiles_without_feature() {
        // With default features (no `embeddings`), EmbeddingRanker exists as
        // a stub that returns no candidates.
        let mut r = EmbeddingRanker::default();
        let lines = vec![KeptLine {
            command: "git",
            line: "anything",
        }];
        assert!(r.rank(&lines).is_empty());
    }
}
