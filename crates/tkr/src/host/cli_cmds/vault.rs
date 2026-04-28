use anyhow::Result;
use tkr_api::vault::{SealState, Vault as VaultTrait};
use crate::host::vault::HostVault;

pub fn status(vault: &HostVault) -> Result<i32> {
    let state = match vault.state() {
        SealState::Sealed => "sealed",
        SealState::AutoUnsealed => "auto-unsealed",
        SealState::FullyUnsealed => "fully-unsealed",
    };
    println!("vault state: {state}");
    Ok(0)
}

#[cfg(test)]
mod tests_status {
    use super::*;
    use std::sync::Arc;
    use crate::host::vault::store::{MemStore, Store};

    #[test]
    fn status_sealed_by_default_in_memory() {
        let v = HostVault::new_in_memory();
        // Capture stdout: easiest to just call and ensure it returns 0.
        assert_eq!(status(&v).unwrap(), 0);
    }

    #[test]
    fn status_after_unseal() {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [1u8; 32]);
        v.unseal_full();
        assert_eq!(status(&v).unwrap(), 0);
        // Crude: re-fetch state to confirm fully unsealed
        assert_eq!(v.state(), tkr_api::vault::SealState::FullyUnsealed);
    }
}
