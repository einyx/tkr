use std::collections::BTreeMap;
use std::path::Path;
use anyhow::{anyhow, Result};
use jsonschema::JSONSchema;
use serde::Deserialize;
use serde_json::Value;
use tkr_api::capability::CapSet;
use tkr_api::manifest::Manifest;

/// Per-plugin section: { grants: [...], config: { ... } }
#[derive(Debug, Default, Deserialize)]
pub struct PluginSection {
    #[serde(default)]
    pub grants: Vec<String>,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Default, Deserialize)]
pub struct HostConfig {
    #[serde(default)]
    pub plugin: BTreeMap<String, PluginSection>,
}

impl HostConfig {
    pub fn from_str(toml_text: &str) -> Result<Self> {
        Ok(toml::from_str(toml_text)?)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(path)?;
        Self::from_str(&s)
    }

    pub fn plugin(&self, name: &str) -> Option<&PluginSection> {
        self.plugin.get(name)
    }

    pub fn grants_for(&self, name: &str) -> CapSet {
        self.plugin
            .get(name)
            .map(|s| s.grants.clone().into())
            .unwrap_or_default()
    }

    /// Validate `[plugin.<name>.config]` against the plugin's declared JSON schema.
    /// Returns `Ok(())` on success, `Err` describing the field+detail on schema violation.
    pub fn validate_config(&self, manifest: &Manifest) -> Result<()> {
        let section = match self.plugin.get(&manifest.name) {
            Some(s) => s,
            None => return Ok(()), // no section means default config; let plugin handle
        };
        if section.config.is_null() {
            return Ok(());
        }
        let schema = &manifest.config_schema;
        if schema.is_null() || schema == &serde_json::json!({}) {
            return Ok(());
        }
        let compiled = JSONSchema::compile(schema)
            .map_err(|e| anyhow!("plugin {}: invalid config_schema: {}", manifest.name, e))?;
        let result = compiled.validate(&section.config);
        if let Err(errors) = result {
            let mut msgs = Vec::new();
            for err in errors {
                msgs.push(format!("{}: {}", err.instance_path, err));
            }
            return Err(anyhow!(
                "plugin {}: config schema mismatch: {}",
                manifest.name,
                msgs.join("; ")
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loads_plugin_section() {
        let cfg = HostConfig::from_str(
            r#"
            [plugin.burn]
            grants = ["cap:vault.read.public"]
            [plugin.burn.config]
            currency = "USD"
        "#,
        )
        .unwrap();
        let burn = cfg.plugin("burn").unwrap();
        assert!(burn.grants.contains(&"cap:vault.read.public".into()));
        assert_eq!(burn.config["currency"], json!("USD"));
        assert!(cfg.grants_for("burn").holds("cap:vault.read.public"));
    }

    #[test]
    fn validates_against_schema() {
        let cfg = HostConfig::from_str(
            r#"
            [plugin.burn.config]
            currency = "USD"
        "#,
        )
        .unwrap();
        let manifest = Manifest {
            name: "burn".into(),
            version: "0".into(),
            config_schema: json!({
                "type": "object",
                "properties": { "currency": { "type": "string" } },
                "required": ["currency"]
            }),
            ..Default::default()
        };
        cfg.validate_config(&manifest).unwrap(); // ok
    }

    #[test]
    fn schema_mismatch_errors_with_field() {
        let cfg = HostConfig::from_str(
            r#"
            [plugin.burn.config]
            currency = 1
        "#,
        )
        .unwrap();
        let manifest = Manifest {
            name: "burn".into(),
            version: "0".into(),
            config_schema: json!({
                "type": "object",
                "properties": { "currency": { "type": "string" } }
            }),
            ..Default::default()
        };
        let err = cfg.validate_config(&manifest).unwrap_err();
        let s = format!("{err}");
        assert!(s.contains("currency") || s.contains("string"));
    }

    #[test]
    fn missing_section_is_ok() {
        let cfg = HostConfig::default();
        let m = Manifest {
            name: "x".into(),
            version: "0".into(),
            ..Default::default()
        };
        cfg.validate_config(&m).unwrap();
    }
}
