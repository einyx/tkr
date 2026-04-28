mod backend;
mod cosine;

use backend::Backend;
use cosine::cosine_similarity;
use std::collections::VecDeque;
use tkr_api::{FilterResult, Plugin};

pub struct SemanticPlugin {
    backend: Backend,
    dedup_threshold: f32,
    relevance_threshold: f32,
    window_size: usize,
    emit_summaries: bool,
    intent_vec: Option<Vec<f32>>,
    window: VecDeque<Vec<f32>>,
    exact_window: VecDeque<String>,
    dedup_count: u64,
    relevance_count: u64,
}

#[derive(Clone)]
pub struct SemanticConfig {
    pub dedup_threshold: f32,
    pub relevance_threshold: f32,
    pub window_size: usize,
    pub emit_summaries: bool,
    pub ollama_url: String,
    pub ollama_model: String,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self {
            dedup_threshold: 0.92,
            relevance_threshold: 0.15,
            window_size: 200,
            emit_summaries: true,
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "nomic-embed-text".into(),
        }
    }
}

impl SemanticPlugin {
    pub fn new(config: &SemanticConfig) -> Self {
        let backend = Backend::new(&config.ollama_url, &config.ollama_model);
        Self {
            backend,
            dedup_threshold: config.dedup_threshold,
            relevance_threshold: config.relevance_threshold,
            window_size: config.window_size,
            emit_summaries: config.emit_summaries,
            intent_vec: None,
            window: VecDeque::new(),
            exact_window: VecDeque::new(),
            dedup_count: 0,
            relevance_count: 0,
        }
    }

    pub fn set_intent(&mut self, command: &str, args: &str) {
        let intent_text = format!("{command} {args}");
        self.intent_vec = self.backend.embed(&intent_text);
    }
}

impl Plugin for SemanticPlugin {
    fn init(_config: &str) -> Box<dyn Plugin> where Self: Sized {
        Box::new(SemanticPlugin::new(&SemanticConfig::default()))
    }

    fn filter(&mut self, line: &str, command: &str, args: &str, index: u64) -> FilterResult {
        if index == 0 { self.set_intent(command, args); }

        if let Some(embedding) = self.backend.embed(line) {
            if let Some(intent) = &self.intent_vec {
                let score = cosine_similarity(&embedding, intent);
                if score < self.relevance_threshold {
                    self.relevance_count += 1;
                    return FilterResult::SuppressWithNote(1);
                }
            }
            for prev in &self.window {
                if cosine_similarity(&embedding, prev) > self.dedup_threshold {
                    self.dedup_count += 1;
                    return FilterResult::SuppressWithNote(0);
                }
            }
            if self.window.len() >= self.window_size { self.window.pop_front(); }
            self.window.push_back(embedding);
        } else {
            if self.exact_window.contains(&line.to_string()) {
                self.dedup_count += 1;
                return FilterResult::SuppressWithNote(0);
            }
            if self.exact_window.len() >= self.window_size { self.exact_window.pop_front(); }
            self.exact_window.push_back(line.to_string());
        }

        FilterResult::Pass
    }

    fn flush(&mut self) -> String {
        if !self.emit_summaries { return String::new(); }
        let mut out = String::new();
        if self.dedup_count > 0 {
            out.push_str(&format!("(+{} similar lines suppressed)\n", self.dedup_count));
        }
        if self.relevance_count > 0 {
            out.push_str(&format!("({} lines below relevance threshold)\n", self.relevance_count));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> SemanticPlugin {
        SemanticPlugin::new(&SemanticConfig {
            dedup_threshold: 0.99,
            relevance_threshold: 0.0,
            ..SemanticConfig::default()
        })
    }

    #[test]
    fn exact_dedup_suppresses_duplicate() {
        let mut p = make();
        assert_eq!(p.filter("warning: foo", "cargo", "build", 0), FilterResult::Pass);
        assert_eq!(p.filter("warning: foo", "cargo", "build", 1), FilterResult::SuppressWithNote(0));
    }

    #[test]
    fn exact_dedup_passes_unique_lines() {
        let mut p = make();
        assert_eq!(p.filter("line a", "git", "status", 0), FilterResult::Pass);
        assert_eq!(p.filter("line b", "git", "status", 1), FilterResult::Pass);
        assert_eq!(p.filter("line c", "git", "status", 2), FilterResult::Pass);
    }

    #[test]
    fn flush_emits_summary_when_dedup_happened() {
        let mut p = make();
        p.filter("same", "git", "log", 0);
        p.filter("same", "git", "log", 1);
        let summary = p.flush();
        assert!(summary.contains("similar lines suppressed"));
    }
}
