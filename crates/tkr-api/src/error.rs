/// All errors that can be produced by the tkr plugin contract v2.
#[derive(Debug)]
pub enum Error {
    UnknownMethod(String),
    CapabilityDenied { cap: String, plugin: String },
    Sealed,
    SchemaMismatch { plugin: String, field: String, detail: String },
    Vault(String),
    Plugin(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::UnknownMethod(m) => write!(f, "unknown method: {m}"),
            Error::CapabilityDenied { cap, plugin } => {
                write!(f, "capability '{cap}' denied for plugin '{plugin}'")
            }
            Error::Sealed => write!(f, "vault is sealed"),
            Error::SchemaMismatch { plugin, field, detail } => {
                write!(f, "schema mismatch in plugin '{plugin}', field '{field}': {detail}")
            }
            Error::Vault(msg) => write!(f, "vault error: {msg}"),
            Error::Plugin(msg) => write!(f, "plugin error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
