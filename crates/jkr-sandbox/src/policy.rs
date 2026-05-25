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
    /// Resource limits + output/timeout caps. All `Option<u64>`; `None` means
    /// "do not constrain." Defaults are deliberately permissive so callers
    /// must opt in to limits.
    #[serde(default)]
    pub limits: SandboxLimits,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SandboxLimits {
    /// RLIMIT_AS (max virtual memory) in bytes. Linux only.
    pub memory_bytes: Option<u64>,
    /// RLIMIT_CPU (max CPU seconds consumed). Linux only. Soft limit kills with
    /// SIGXCPU; the kernel sends SIGKILL on the hard limit one second later.
    pub cpu_seconds: Option<u64>,
    /// RLIMIT_FSIZE (max single-file size the child can create) in bytes.
    /// Linux only.
    pub file_size_bytes: Option<u64>,
    /// Cap on total stdout+stderr bytes captured. Above this, output is
    /// truncated and the child is killed. Default applied by exec: 16 MiB.
    pub max_output_bytes: Option<u64>,
    /// Wall-clock timeout in milliseconds. Above this, the child is killed
    /// and run returns `SandboxError::Timeout`.
    pub timeout_ms: Option<u64>,
    /// Network policy. Default = inherit (no restriction).
    #[serde(default)]
    pub network: NetworkPolicy,
}

/// Network restrictions. Linux uses landlock ABI v4 (Linux 6.7+) for TCP
/// connect/bind rules; older kernels silently ignore these and the run
/// proceeds without enforcement. macOS uses sandbox-exec's `(deny network*)`
/// with port carve-outs in the generated .sb profile.
///
/// **Caveats** (port-only enforcement at the kernel level):
/// - No host allowlist. Cannot say "only api.anthropic.com:443" — landlock
///   filters by port, not destination IP/host. For host filtering you need
///   a SOCKS proxy or a network namespace.
/// - UDP, raw sockets, and Unix-domain sockets are NOT covered by landlock's
///   network access controls. Those flow through regardless.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum NetworkPolicy {
    /// No restriction (default).
    #[default]
    Inherit,
    /// Block all outbound TCP connect + TCP bind. UDP not covered (see above).
    DenyAll,
    /// Block by default, allow listed TCP ports.
    Allow {
        #[serde(default)]
        connect_ports: Vec<u16>,
        #[serde(default)]
        bind_ports: Vec<u16>,
    },
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
    pub fn memory_bytes(mut self, n: u64) -> Self {
        self.inner.limits.memory_bytes = Some(n);
        self
    }
    pub fn cpu_seconds(mut self, n: u64) -> Self {
        self.inner.limits.cpu_seconds = Some(n);
        self
    }
    pub fn file_size_bytes(mut self, n: u64) -> Self {
        self.inner.limits.file_size_bytes = Some(n);
        self
    }
    pub fn max_output_bytes(mut self, n: u64) -> Self {
        self.inner.limits.max_output_bytes = Some(n);
        self
    }
    pub fn timeout_ms(mut self, n: u64) -> Self {
        self.inner.limits.timeout_ms = Some(n);
        self
    }
    pub fn deny_network(mut self) -> Self {
        self.inner.limits.network = NetworkPolicy::DenyAll;
        self
    }
    pub fn allow_tcp_connect(mut self, port: u16) -> Self {
        self.inner.limits.network = match std::mem::take(&mut self.inner.limits.network) {
            NetworkPolicy::Allow {
                mut connect_ports,
                bind_ports,
            } => {
                connect_ports.push(port);
                NetworkPolicy::Allow {
                    connect_ports,
                    bind_ports,
                }
            }
            _ => NetworkPolicy::Allow {
                connect_ports: vec![port],
                bind_ports: Vec::new(),
            },
        };
        self
    }
    pub fn allow_tcp_bind(mut self, port: u16) -> Self {
        self.inner.limits.network = match std::mem::take(&mut self.inner.limits.network) {
            NetworkPolicy::Allow {
                connect_ports,
                mut bind_ports,
            } => {
                bind_ports.push(port);
                NetworkPolicy::Allow {
                    connect_ports,
                    bind_ports,
                }
            }
            _ => NetworkPolicy::Allow {
                connect_ports: Vec::new(),
                bind_ports: vec![port],
            },
        };
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
