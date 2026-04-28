use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub core: CoreConfig,
    pub plugins: PluginsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CoreConfig {
    pub plugin_dir: String,
    pub socket_path: String,
    pub filter_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    pub chain: Vec<String>,
    pub semantic: SemanticConfig,
    pub analytics: AnalyticsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SemanticConfig {
    pub dedup_threshold: f32,
    pub relevance_threshold: f32,
    pub window_size: usize,
    pub emit_summaries: bool,
    pub ollama_url: String,
    pub ollama_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalyticsConfig {
    pub db_path: String,
}

impl Default for Config {
    fn default() -> Self { Self { core: CoreConfig::default(), plugins: PluginsConfig::default() } }
}

impl Default for CoreConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            plugin_dir: home.join(".tkr/plugins").to_string_lossy().into(),
            socket_path: home.join(".tkr/session.sock").to_string_lossy().into(),
            filter_dir: home.join(".tkr/filters").to_string_lossy().into(),
        }
    }
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            // Analytics is recorded by proxy::run from the canonical PipelineResult,
            // not via the plugin chain (suppressed lines short-circuit before reaching plugins).
            chain: vec!["tkr-filter".into(), "tkr-semantic".into()],
            semantic: SemanticConfig::default(),
            analytics: AnalyticsConfig::default(),
        }
    }
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

impl Default for AnalyticsConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self { db_path: home.join(".tkr/analytics.db").to_string_lossy().into() }
    }
}

pub fn load() -> Result<Config> {
    let path = config_path();
    if !path.exists() { return Ok(Config::default()); }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn config_path() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".tkr/config.toml")
}

pub fn bundled_filters_dir() -> Option<PathBuf> {
    option_env!("TKR_BUNDLED_FILTERS_DIR").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_filter_and_semantic_plugins() {
        let cfg = Config::default();
        assert_eq!(cfg.plugins.chain, vec!["tkr-filter", "tkr-semantic"]);
    }

    #[test]
    fn parse_minimal_toml() {
        let toml_str = r#"
[core]
plugin_dir = "/tmp/plugins"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.core.plugin_dir, "/tmp/plugins");
    }
}
