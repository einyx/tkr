//! jkr plugin contract.
//!
//! v1 (legacy C-ABI line-filter) types live under `legacy::*` and are
//! re-exported here for backwards compatibility while v2 is built out.
pub mod legacy;
pub use legacy::{FilterResult, Plugin as LegacyPlugin};

pub mod error;
pub use error::{Error, Result};

pub mod manifest;
pub use manifest::SensitivityClass;

pub mod agent_event;
pub use agent_event::{AgentEvent, AgentEventKind};

pub mod bus;
pub mod capability;
pub mod handles;
pub mod host;
pub mod plugin;
pub mod vault;
pub use plugin::{CommandCtx, FilterDecision, Plugin};

#[cfg(feature = "test-host")]
pub mod test_host;
