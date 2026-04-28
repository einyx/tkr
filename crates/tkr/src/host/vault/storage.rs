use std::sync::Arc;

use serde_json::Value;
use tkr_api::manifest::SensitivityClass;
use tkr_api::{Error, Result};

use crate::host::vault::HostVault;

// ─── KvImpl ──────────────────────────────────────────────────────────────────

pub struct KvImpl {
    vault: Arc<HostVault>,
    plugin: String,
    class: SensitivityClass,
}

impl KvImpl {
    pub fn new(
        vault: Arc<HostVault>,
        plugin: impl Into<String>,
        class: SensitivityClass,
    ) -> Self {
        Self {
            vault,
            plugin: plugin.into(),
            class,
        }
    }

    fn key(&self, k: &str) -> String {
        format!("kv/{}/{}", self.plugin, k)
    }
}

impl tkr_api::handles::Kv for KvImpl {
    fn get(&self, key: &str) -> Result<Option<Value>> {
        match self
            .vault
            .read(self.class, &self.key(key), &self.plugin)
            .map_err(|e| Error::Vault(e.to_string()))?
        {
            None => Ok(None),
            Some(z) => {
                let v: Value = serde_json::from_slice(&z[..])
                    .map_err(|e| Error::Vault(e.to_string()))?;
                Ok(Some(v))
            }
        }
    }

    fn put(&self, key: &str, val: Value) -> Result<()> {
        let bytes =
            serde_json::to_vec(&val).map_err(|e| Error::Vault(e.to_string()))?;
        self.vault
            .write(self.class, &self.key(key), &bytes, &self.plugin)
            .map_err(|e| Error::Vault(e.to_string()))
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.vault
            .delete(self.class, &self.key(key))
            .map_err(|e| Error::Vault(e.to_string()))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let full = format!("kv/{}/{}", self.plugin, prefix);
        let raw = self
            .vault
            .list(self.class, &full)
            .map_err(|e| Error::Vault(e.to_string()))?;
        let strip = format!("kv/{}/", self.plugin);
        Ok(raw
            .into_iter()
            .filter_map(|k| k.strip_prefix(&strip).map(|s| s.to_string()))
            .collect())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_kv {
    use super::*;
    use crate::host::vault::store::{MemStore, Store};
    use std::sync::Arc;
    use tkr_api::handles::Kv;
    use tkr_api::manifest::SensitivityClass;

    fn vault() -> Arc<HostVault> {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [9u8; 32]);
        v.unseal_full();
        Arc::new(v)
    }

    #[test]
    fn kv_round_trip() {
        let kv = KvImpl::new(vault(), "demo", SensitivityClass::Public);
        kv.put("k", serde_json::json!({"n":1})).unwrap();
        assert_eq!(kv.get("k").unwrap(), Some(serde_json::json!({"n":1})));
    }

    #[test]
    fn kv_namespaced_per_plugin() {
        let v = vault();
        let a = KvImpl::new(v.clone(), "alpha", SensitivityClass::Public);
        let b = KvImpl::new(v.clone(), "beta", SensitivityClass::Public);
        a.put("k", serde_json::json!(1)).unwrap();
        assert_eq!(b.get("k").unwrap(), None);
    }

    #[test]
    fn kv_list_with_prefix() {
        let kv = KvImpl::new(vault(), "demo", SensitivityClass::Public);
        kv.put("a/x", serde_json::json!(1)).unwrap();
        kv.put("a/y", serde_json::json!(2)).unwrap();
        kv.put("b/z", serde_json::json!(3)).unwrap();
        let mut listed = kv.list("a/").unwrap();
        listed.sort();
        assert_eq!(listed, vec!["a/x".to_string(), "a/y".to_string()]);
    }
}
