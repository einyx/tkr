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
    #[serde(default)]
    pub sandbox: Option<SandboxDecl>,
    #[serde(default)]
    pub secrets: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct ModelDecl {
    pub provider: String,
    pub name: String,
}

fn default_tool_config() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

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

fn default_mode() -> AgentMode {
    AgentMode::Approve
}
fn default_max_steps() -> u32 {
    20
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct SandboxDecl {
    #[serde(default)]
    pub backend: SandboxBackend,
    pub image: String,
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default)]
    pub workspace: WorkspaceMode,
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u64,
    #[serde(default = "default_timeout_s")]
    pub timeout_s: u64,
}

#[derive(Debug, Deserialize, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackend {
    #[default]
    Landlock,
    Docker,
}

#[derive(Debug, Deserialize, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkPolicy {
    None,
    #[default]
    GatewayOnly,
    Open,
}

#[derive(Debug, Deserialize, PartialEq, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    Ro,
    #[default]
    Rw,
}

fn default_memory_mb() -> u64 { 512 }
fn default_timeout_s() -> u64 { 600 }

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

    #[test]
    fn parses_optional_sandbox_and_secrets() {
        let toml = r#"
            name = "explorer"
            task = "{{input}}"
            mode = "auto"
            max_steps = 30
            [model]
            provider = "anthropic"
            name = "claude-sonnet-4-6"
            [sandbox]
            backend = "docker"
            image = "jkr-agent:latest"
            network = "gateway-only"
            workspace = "rw"
            memory_mb = 512
            timeout_s = 600
            [secrets]
            GITHUB_TOKEN = "vault:ci/github-pat"
        "#;
        let m = Manifest::parse(toml).unwrap();
        let sb = m.sandbox.expect("sandbox block");
        assert_eq!(sb.backend, SandboxBackend::Docker);
        assert_eq!(sb.network, NetworkPolicy::GatewayOnly);
        assert_eq!(sb.workspace, WorkspaceMode::Rw);
        assert_eq!(sb.memory_mb, 512);
        assert_eq!(m.secrets.get("GITHUB_TOKEN").unwrap(), "vault:ci/github-pat");
    }

    #[test]
    fn manifest_without_new_blocks_still_parses() {
        let toml = r#"
            name = "echo-bot"
            task = "say hi"
            [model]
            provider = "anthropic"
            name = "claude-sonnet-4-6"
        "#;
        let m = Manifest::parse(toml).unwrap();
        assert!(m.sandbox.is_none());
        assert!(m.secrets.is_empty());
    }

    #[test]
    fn sandbox_defaults_to_landlock_backend() {
        let toml = r#"
            name = "x"
            task = "t"
            [model]
            provider = "anthropic"
            name = "m"
            [sandbox]
            image = "img"
        "#;
        let sb = Manifest::parse(toml).unwrap().sandbox.unwrap();
        assert_eq!(sb.backend, SandboxBackend::Landlock);
        assert_eq!(sb.network, NetworkPolicy::GatewayOnly);
        assert_eq!(sb.workspace, WorkspaceMode::Rw);
    }
}
