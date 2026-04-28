pub mod age_codec;
pub mod audit;
pub mod keychain;
pub mod seal;
pub mod store;

use std::sync::{Arc, Mutex};
use anyhow::{anyhow, Result};
use tkr_api::manifest::SensitivityClass;
use tkr_api::vault::{SealState, Vault as VaultTrait};
use crate::host::vault::audit::{AuditEvent, AuditLog};
use crate::host::vault::seal::SealStateMachine;
use crate::host::vault::store::{MemStore, Store};

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub struct HostVault {
    sm: Mutex<SealStateMachine>,
    store: Arc<dyn Store>,
    audit: Option<AuditLog>,
}

impl HostVault {
    /// Fully sealed, in-memory backing. Tests call `unseal_full()` before writing.
    pub fn new_in_memory() -> Self {
        let zero = [0u8; 32];
        Self {
            sm: Mutex::new(SealStateMachine::sealed(zero)),
            store: Arc::new(MemStore::default()),
            audit: None,
        }
    }

    /// Store-backed vault with auto-unseal (public class available immediately).
    pub fn new(store: Arc<dyn Store>, master: [u8; 32]) -> Self {
        Self {
            sm: Mutex::new(SealStateMachine::auto_unseal(master)),
            store,
            audit: None,
        }
    }

    /// Builder: attach an audit log to this vault.
    pub fn with_audit(mut self, log: AuditLog) -> Self {
        self.audit = Some(log);
        self
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
        let sm = self.sm.lock().unwrap();
        let sub = sm
            .subkey(class)
            .ok_or_else(|| anyhow!("vault sealed for {:?}", class))?;
        let sub = *sub;
        drop(sm);
        let ct = age_codec::encrypt(&sub, val)?;
        self.store.put(&Self::key_for(class, user_key), &ct)?;
        if class == SensitivityClass::Secret {
            if let Some(log) = &self.audit {
                let _ = log.append(AuditEvent {
                    actor: actor.to_string(),
                    op: "write".to_string(),
                    key: user_key.to_string(),
                    ts: now_ts(),
                });
            }
        }
        Ok(())
    }

    pub fn read(&self, class: SensitivityClass, user_key: &str, actor: &str) -> Result<Option<Vec<u8>>> {
        let sm = self.sm.lock().unwrap();
        let sub = sm
            .subkey(class)
            .ok_or_else(|| anyhow!("vault sealed for {:?}", class))?;
        let sub = *sub;
        drop(sm);
        let result = match self.store.get(&Self::key_for(class, user_key))? {
            None => Ok(None),
            Some(ct) => Ok(Some(age_codec::decrypt(&sub, &ct)?)),
        };
        if class == SensitivityClass::Secret {
            if let Some(log) = &self.audit {
                let _ = log.append(AuditEvent {
                    actor: actor.to_string(),
                    op: "read".to_string(),
                    key: user_key.to_string(),
                    ts: now_ts(),
                });
            }
        }
        result
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

#[cfg(test)]
mod tests_persistence {
    use super::*;
    use tempfile::tempdir;
    use crate::host::vault::store::FsStore;

    #[test]
    fn vault_persists_across_reopen() {
        let d = tempdir().unwrap();
        let master = [3u8; 32];
        {
            let store: Arc<dyn Store> = Arc::new(FsStore::new(d.path()).unwrap());
            let v = HostVault::new(store, master);
            v.unseal_full();
            v.write(SensitivityClass::Private, "k", b"hello", "host").unwrap();
        }
        let store: Arc<dyn Store> = Arc::new(FsStore::new(d.path()).unwrap());
        let v = HostVault::new(store, master);
        v.unseal_full();
        assert_eq!(
            v.read(SensitivityClass::Private, "k", "host").unwrap().unwrap(),
            b"hello".to_vec()
        );
    }
}
