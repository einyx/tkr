use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub model: ModelDecl,
    #[serde(default)]
    pub system: Option<String>,
    pub task: String,
    #[serde(default)]
    pub tools: Vec<ToolDecl>,
    #[serde(default = "default_mode")]
    pub mode: AgentMode,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct ModelDecl {
    pub provider: String,
    pub name: String,
}

fn default_tool_config() -> toml::Value { toml::Value::Table(toml::map::Map::new()) }

#[derive(Debug, Deserialize, PartialEq)]
pub struct ToolDecl {
    pub name: String,
    #[serde(default = "default_tool_config")]
    pub config: toml::Value,
}

#[derive(Debug, Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    DryRun,
    Approve,
    Auto,
}

fn default_mode() -> AgentMode { AgentMode::Approve }
fn default_max_steps() -> u32 { 20 }

impl Manifest {
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(input)?)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let src = r#"
name = "hello"
task = "say hi"

[model]
provider = "anthropic"
name = "claude-sonnet-4-6"
"#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.name, "hello");
        assert_eq!(m.task, "say hi");
        assert_eq!(m.model.provider, "anthropic");
        assert_eq!(m.mode, AgentMode::Approve);
        assert_eq!(m.max_steps, 20);
        assert!(m.tools.is_empty());
    }

    #[test]
    fn parses_tools_and_mode() {
        let src = r#"
name = "loud"
task = "echo something"
mode = "auto"
max_steps = 5

[model]
provider = "anthropic"
name = "claude-sonnet-4-6"

[[tools]]
name = "echo"

[[tools]]
name = "echo"
[tools.config]
prefix = "!"
"#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.mode, AgentMode::Auto);
        assert_eq!(m.max_steps, 5);
        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.tools[0].name, "echo");
    }

    #[test]
    fn rejects_missing_required_fields() {
        let src = r#"name = "x""#;
        assert!(Manifest::parse(src).is_err());
    }
}
