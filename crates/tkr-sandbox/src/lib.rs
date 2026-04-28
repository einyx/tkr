pub mod error;
pub mod policy;
pub mod exec;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

pub use error::SandboxError;
pub use policy::{SandboxPolicy, PolicyBuilder};
pub use exec::{run_sandboxed, SandboxOutput};
