use crate::config::Config;
use anyhow::Result;
use tkr_api::Plugin;
use tkr_analytics::AnalyticsPlugin;
use tkr_filter::FilterPlugin;
use tkr_semantic::{SemanticConfig, SemanticPlugin};

pub fn build_chain(cfg: &Config, _command: &str) -> Result<Vec<Box<dyn Plugin>>> {
    let mut chain: Vec<Box<dyn Plugin>> = Vec::new();

    for plugin_name in &cfg.plugins.chain {
        match plugin_name.as_str() {
            "tkr-filter" => {
                let user_dir = std::path::Path::new(&cfg.core.filter_dir);
                let mut plugin = FilterPlugin::from_dir(user_dir)
                    .unwrap_or_else(|_| FilterPlugin::from_toml("").unwrap());
                if let Some(bundled) = crate::config::bundled_filters_dir() {
                    if bundled.is_dir() {
                        if let Ok(p) = FilterPlugin::from_dir(&bundled) { plugin = p; }
                    }
                }
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
                let db = &cfg.plugins.analytics.db_path;
                std::fs::create_dir_all(
                    std::path::Path::new(db).parent().unwrap_or(std::path::Path::new(".")),
                ).ok();
                if let Ok(p) = AnalyticsPlugin::new(db) { chain.push(Box::new(p)); }
            }
            other => {
                eprintln!("tkr: unknown built-in plugin '{other}', skipping");
            }
        }
    }
    Ok(chain)
}
