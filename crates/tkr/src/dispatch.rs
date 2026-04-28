// Legacy filter-chain builder kept for reference; no longer called — the v2
// PluginRegistry (host::boot) is the canonical filter chain since Task 6.3.
// This module is kept to avoid breaking any external code that may reference it,
// but build_chain is now dead code.

use crate::config::Config;
use anyhow::Result;
use tkr_api::LegacyPlugin as Plugin;
use tkr_filter::FilterPlugin;
use tkr_semantic::{SemanticConfig, SemanticPlugin};

#[allow(dead_code)]
pub fn build_chain(cfg: &Config, _command: &str) -> Result<Vec<Box<dyn Plugin>>> {
    let mut chain: Vec<Box<dyn Plugin>> = Vec::new();

    for plugin_name in &cfg.plugins.chain {
        match plugin_name.as_str() {
            "tkr-filter" => {
                let mut plugin = FilterPlugin::from_toml("").unwrap();
                if let Some(bundled) = crate::config::bundled_filters_dir() {
                    let _ = plugin.load_dir(&bundled);
                }
                let user_dir = std::path::Path::new(&cfg.core.filter_dir);
                let _ = plugin.load_dir(user_dir);
                chain.push(Box::new(plugin));
            }
            "tkr-semantic" => {
                let sc = SemanticConfig {
                    dedup_threshold: cfg.plugins.semantic.dedup_threshold,
                    relevance_threshold: cfg.plugins.semantic.relevance_threshold,
                    window_size: cfg.plugins.semantic.window_size,
                    emit_summaries: cfg.plugins.semantic.emit_summaries,
                    ollama_url: cfg.plugins.semantic.ollama_url.clone(),
                    ollama_model: cfg.plugins.semantic.ollama_model.clone(),
                };
                chain.push(Box::new(SemanticPlugin::new(&sc)));
            }
            "tkr-analytics" => {
                // AnalyticsPlugin (legacy) removed — analytics now handled by
                // AnalyticsPluginV2 in the v2 PluginRegistry (host::boot).
                // This arm is a no-op to avoid panicking if config still lists it.
            }
            other => {
                eprintln!("tkr: unknown built-in plugin '{other}', skipping");
            }
        }
    }
    Ok(chain)
}
