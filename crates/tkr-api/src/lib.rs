//! tkr plugin contract.
//!
//! v1 (legacy C-ABI line-filter) types live under `legacy::*` and are
//! re-exported here for backwards compatibility while v2 is built out.
pub mod legacy;
pub use legacy::{Plugin as LegacyPlugin, FilterResult, LineContext};

pub mod error;
pub use error::{Error, Result};

pub mod manifest;
pub use manifest::SensitivityClass;

pub mod capability;
pub mod bus;
pub mod vault;
// Re-export every other public symbol that legacy.rs defines so that
// existing consumers compile without changes.
pub use legacy::Plugin;
pub use legacy::{FnInit, FnFilter, FnFlush, FnDestroy};
