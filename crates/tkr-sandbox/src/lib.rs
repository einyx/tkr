pub mod error;
pub mod exec;
pub mod policy;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

pub use error::SandboxError;
pub use exec::{run_sandboxed, SandboxOutput};
pub use policy::{PolicyBuilder, SandboxPolicy};
