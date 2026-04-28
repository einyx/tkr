use anyhow::Result;
use tkr_api::manifest::SensitivityClass;
use crate::host::vault::HostVault;

pub fn reset(vault: &HostVault, plugin: &str) -> Result<i32> {
    vault.unseal_full();

    let prefixes = ["kv/", "fs/", "sqlite/"];
    let mut deleted = 0usize;
    for class in [SensitivityClass::Public, SensitivityClass::Private, SensitivityClass::Secret] {
        for prefix in &prefixes {
            let full_prefix = format!("{prefix}{plugin}/");
            let keys = vault.list(class, &full_prefix)?;
            for k in keys {
                vault.delete(class, &k)?;
                deleted += 1;
            }
        }
    }
    println!("deleted {deleted} vault entries owned by plugin {plugin}");
    Ok(0)
}

#[cfg(test)]
mod tests_reset {
    use super::*;
    use std::sync::Arc;
    use crate::host::vault::store::{MemStore, Store};

    #[test]
    fn reset_deletes_only_target_plugin_entries() {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [9u8; 32]);
        v.unseal_full();

        // Write entries from two plugins under multiple namespaces and classes.
        v.write(SensitivityClass::Public, "kv/alpha/x", b"1", "alpha").unwrap();
        v.write(SensitivityClass::Public, "fs/alpha/y", b"2", "alpha").unwrap();
        v.write(SensitivityClass::Private, "kv/alpha/z", b"3", "alpha").unwrap();
        v.write(SensitivityClass::Public, "kv/beta/x", b"keep", "beta").unwrap();

        reset(&v, "alpha").unwrap();

        // alpha entries gone:
        assert!(v.read(SensitivityClass::Public, "kv/alpha/x", "alpha").unwrap().is_none());
        assert!(v.read(SensitivityClass::Public, "fs/alpha/y", "alpha").unwrap().is_none());
        assert!(v.read(SensitivityClass::Private, "kv/alpha/z", "alpha").unwrap().is_none());
        // beta entry untouched:
        let z = v.read(SensitivityClass::Public, "kv/beta/x", "beta").unwrap().unwrap();
        assert_eq!(&z[..], b"keep");
    }

    #[test]
    fn reset_unknown_plugin_is_noop() {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [1u8; 32]);
        v.unseal_full();
        let r = reset(&v, "ghost").unwrap();
        assert_eq!(r, 0);
    }
}
