use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SandboxPolicy {
    #[serde(default)]
    pub fs_read: Vec<PathBuf>,
    #[serde(default)]
    pub fs_write: Vec<PathBuf>,
    #[serde(default)]
    pub disabled: bool,
    /// Environment variables forwarded to the sandboxed child. The default
    /// (empty) means the child starts with **no** inherited environment —
    /// secrets like `ANTHROPIC_API_KEY`, `AWS_*`, `GITHUB_TOKEN` cannot leak
    /// across the boundary. Names listed here are looked up in the parent
    /// env at spawn time and forwarded only if present.
    #[serde(default)]
    pub env_allow: Vec<String>,
}

impl SandboxPolicy {
    pub fn deny_all() -> Self {
        Self::default()
    }
    pub fn builder() -> PolicyBuilder {
        PolicyBuilder::default()
    }
    pub fn validate(&self) -> Result<(), String> {
        for p in self.fs_read.iter().chain(self.fs_write.iter()) {
            if !p.is_absolute() {
                return Err(format!("policy paths must be absolute: {}", p.display()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct PolicyBuilder {
    inner: SandboxPolicy,
}

impl PolicyBuilder {
    pub fn allow_read<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.inner.fs_read.push(p.as_ref().to_path_buf());
        self
    }
    pub fn allow_write<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.inner.fs_write.push(p.as_ref().to_path_buf());
        self
    }
    pub fn allow_env<S: Into<String>>(mut self, name: S) -> Self {
        self.inner.env_allow.push(name.into());
        self
    }
    pub fn build(self) -> SandboxPolicy {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_all_is_empty() {
        let p = SandboxPolicy::deny_all();
        assert!(p.fs_read.is_empty() && p.fs_write.is_empty() && !p.disabled);
    }

    #[test]
    fn builder_chains() {
        let p = SandboxPolicy::builder()
            .allow_read("/etc")
            .allow_write("/tmp/foo")
            .build();
        assert_eq!(p.fs_read, vec![PathBuf::from("/etc")]);
        assert_eq!(p.fs_write, vec![PathBuf::from("/tmp/foo")]);
    }

    #[test]
    fn validate_rejects_relative() {
        let p = SandboxPolicy::builder().allow_read("relative/path").build();
        assert!(p.validate().is_err());
    }

    #[test]
    fn deserializes_from_toml() {
        let src = r#"fs_read = ["/etc"]
fs_write = ["/tmp"]"#;
        let p: SandboxPolicy = toml::from_str(src).unwrap();
        assert_eq!(p.fs_read.len(), 1);
        assert_eq!(p.fs_write.len(), 1);
    }
}
