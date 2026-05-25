pub mod capture;
pub mod error;
pub mod exec;
pub mod policy;

pub use capture::trace::{SandboxTrace, Verdict, VerdictLevel};

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(feature = "docker")]
pub mod session;

pub use error::SandboxError;
pub use exec::{
    run_sandboxed, run_sandboxed_interactive, run_sandboxed_output_only, SandboxOutput,
};
pub use policy::{PolicyBuilder, SandboxPolicy};
