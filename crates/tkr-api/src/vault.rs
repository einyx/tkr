use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SealState {
    #[default]
    Sealed,
    AutoUnsealed,
    FullyUnsealed,
}

pub trait Vault: Send + Sync {
    fn state(&self) -> SealState;
    fn read_secret(&self, key: &str) -> Result<Vec<u8>>;
    fn write_secret(&self, key: &str, val: &[u8]) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_state_ordering() {
        assert!(SealState::Sealed < SealState::AutoUnsealed);
        assert!(SealState::AutoUnsealed < SealState::FullyUnsealed);
    }
}
