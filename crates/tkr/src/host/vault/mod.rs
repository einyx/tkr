pub mod age_codec;
pub mod keychain;
pub mod seal;
pub mod store;

use std::sync::{Arc, Mutex};
use anyhow::{anyhow, Result};
use tkr_api::manifest::SensitivityClass;
use tkr_api::vault::{SealState, Vault as VaultTrait};
use crate::host::vault::seal::SealStateMachine;
use crate::host::vault::store::{MemStore, Store};

pub struct HostVault {
    sm: Mutex<SealStateMachine>,
    store: Arc<dyn Store>,
}

impl HostVault {
    /// Fully sealed, in-memory backing. Tests call `unseal_full()` before writing.
    pub fn new_in_memory() -> Self {
        let zero = [0u8; 32];
        Self {
            sm: Mutex::new(SealStateMachine::sealed(zero)),
            store: Arc::new(MemStore::default()),
        }
    }

    /// In-memory backing with auto-unseal (public class available immediately).
    pub fn new(store: Arc<dyn Store>, master: [u8; 32]) -> Self {
        Self {
            sm: Mutex::new(SealStateMachine::auto_unseal(master)),
            store,
        }
    }

    pub fn unseal_full(&self) {
        self.sm.lock().unwrap().full_unseal();
    }

    pub fn seal(&self) {
        self.sm.lock().unwrap().seal();
    }

    fn class_prefix(class: SensitivityClass) -> &'static str {
        match class {
            SensitivityClass::Public => "public",
            SensitivityClass::Private => "private",
            SensitivityClass::Secret => "secret",
        }
    }

    fn key_for(class: SensitivityClass, user_key: &str) -> String {
        format!("{}/{user_key}", Self::class_prefix(class))
    }

    pub fn write(&self, class: SensitivityClass, user_key: &str, val: &[u8], actor: &str) -> Result<()> {
        let _ = actor; // reserved for audit (Task 2.7)
        let sm = self.sm.lock().unwrap();
        let sub = sm
            .subkey(class)
            .ok_or_else(|| anyhow!("vault sealed for {:?}", class))?;
        let sub = *sub;
        drop(sm);
        let ct = age_codec::encrypt(&sub, val)?;
        self.store.put(&Self::key_for(class, user_key), &ct)
    }

    pub fn read(&self, class: SensitivityClass, user_key: &str, actor: &str) -> Result<Option<Vec<u8>>> {
        let _ = actor; // reserved for audit (Task 2.7)
        let sm = self.sm.lock().unwrap();
        let sub = sm
            .subkey(class)
            .ok_or_else(|| anyhow!("vault sealed for {:?}", class))?;
        let sub = *sub;
        drop(sm);
        match self.store.get(&Self::key_for(class, user_key))? {
            None => Ok(None),
            Some(ct) => Ok(Some(age_codec::decrypt(&sub, &ct)?)),
        }
    }

    pub fn delete(&self, class: SensitivityClass, user_key: &str) -> Result<()> {
        self.store.delete(&Self::key_for(class, user_key))
    }

    pub fn list(&self, class: SensitivityClass, prefix: &str) -> Result<Vec<String>> {
        let full = format!("{}/{}", Self::class_prefix(class), prefix);
        let raw = self.store.list(&full)?;
        let strip = format!("{}/", Self::class_prefix(class));
        Ok(raw
            .into_iter()
            .filter_map(|k| k.strip_prefix(&strip).map(|s| s.to_string()))
            .collect())
    }
}

impl VaultTrait for HostVault {
    fn state(&self) -> SealState {
        self.sm.lock().unwrap().state()
    }

    fn read_secret(&self, key: &str) -> tkr_api::Result<Vec<u8>> {
        self.read(SensitivityClass::Secret, key, "host")
            .map_err(|e| {
                if e.to_string().contains("vault sealed") {
                    tkr_api::Error::Sealed
                } else {
                    tkr_api::Error::Vault(e.to_string())
                }
            })?
            .ok_or_else(|| tkr_api::Error::Vault(format!("missing key {key}")))
    }

    fn write_secret(&self, key: &str, val: &[u8]) -> tkr_api::Result<()> {
        self.write(SensitivityClass::Secret, key, val, "host")
            .map_err(|e| {
                if e.to_string().contains("vault sealed") {
                    tkr_api::Error::Sealed
                } else {
                    tkr_api::Error::Vault(e.to_string())
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_secret_requires_full_unseal() {
        let v = HostVault::new_in_memory();
        assert_eq!(v.state(), SealState::Sealed);
        assert!(VaultTrait::write_secret(&v, "k", b"x").is_err());
        v.unseal_full();
        VaultTrait::write_secret(&v, "k", b"x").unwrap();
        assert_eq!(VaultTrait::read_secret(&v, "k").unwrap(), b"x".to_vec());
    }

    #[test]
    fn public_read_after_auto_unseal_only() {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [9u8; 32]);
        v.write(SensitivityClass::Public, "x", b"hi", "host").unwrap();
        assert_eq!(
            v.read(SensitivityClass::Public, "x", "host").unwrap().unwrap(),
            b"hi".to_vec()
        );
        assert!(v.write(SensitivityClass::Private, "y", b"z", "host").is_err());
        v.unseal_full();
        v.write(SensitivityClass::Private, "y", b"z", "host").unwrap();
        assert_eq!(
            v.read(SensitivityClass::Private, "y", "host").unwrap().unwrap(),
            b"z".to_vec()
        );
    }
}
