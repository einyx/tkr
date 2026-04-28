//! tkr plugin contract.
//!
//! v1 (legacy C-ABI line-filter) types live under `legacy::*` and are
//! re-exported here for backwards compatibility while v2 is built out.
pub mod legacy;
pub use legacy::{Plugin as LegacyPlugin, FilterResult};

pub mod error;
pub use error::{Error, Result};

pub mod manifest;
pub use manifest::SensitivityClass;

pub mod capability;
pub mod bus;
pub mod vault;
pub mod handles;
pub mod host;
pub mod plugin;
pub use plugin::{Plugin, CommandCtx, FilterDecision};

#[cfg(feature = "test-host")]
pub mod test_host;
