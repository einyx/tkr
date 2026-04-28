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
}

impl SandboxPolicy {
    pub fn deny_all() -> Self { Self::default() }
    pub fn builder() -> PolicyBuilder { PolicyBuilder::default() }
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
pub struct PolicyBuilder { inner: SandboxPolicy }

impl PolicyBuilder {
    pub fn allow_read<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.inner.fs_read.push(p.as_ref().to_path_buf()); self
    }
    pub fn allow_write<P: AsRef<Path>>(mut self, p: P) -> Self {
        self.inner.fs_write.push(p.as_ref().to_path_buf()); self
    }
    pub fn build(self) -> SandboxPolicy { self.inner }
}
