use std::sync::Arc;
use tkr_api::{Result as ApiResult, Error};
use tkr_api::bus::Bus;
use tkr_api::handles::{Fs, Kv, Sqlite};
use tkr_api::host::Host;
use tkr_api::manifest::SensitivityClass;
use tkr_api::vault::Vault;

use crate::host::bus::InProcBus;
use crate::host::vault::{HostVault, storage::{FsImpl, KvImpl, SqliteImpl}};

pub mod bus;
pub mod loader;
pub mod vault;

/// Per-plugin host. Holds references to the vault and bus, plus pre-built handles.
pub struct RealHost {
    plugin: String,
    vault: Arc<HostVault>,
    bus: Arc<InProcBus>,

    kv_pub: KvImpl,
    kv_priv: KvImpl,
    kv_sec: KvImpl,
    fs_pub: FsImpl,
    fs_priv: FsImpl,
    fs_sec: FsImpl,
    // Sqlite is created on demand via host.sqlite(schema, class). We don't pre-build
    // sqlite handles because each requires a schema_sql at open time. The plugin
    // gets at most one SqliteImpl per class; cache it on first request.
    sqlite_pub: std::sync::OnceLock<SqliteImpl>,
    sqlite_priv: std::sync::OnceLock<SqliteImpl>,
    sqlite_sec: std::sync::OnceLock<SqliteImpl>,
}

impl RealHost {
    pub fn new(plugin: impl Into<String>, vault: Arc<HostVault>, bus: Arc<InProcBus>) -> Self {
        let plugin = plugin.into();
        Self {
            kv_pub: KvImpl::new(vault.clone(), &plugin, SensitivityClass::Public),
            kv_priv: KvImpl::new(vault.clone(), &plugin, SensitivityClass::Private),
            kv_sec: KvImpl::new(vault.clone(), &plugin, SensitivityClass::Secret),
            fs_pub: FsImpl::new(vault.clone(), &plugin, SensitivityClass::Public),
            fs_priv: FsImpl::new(vault.clone(), &plugin, SensitivityClass::Private),
            fs_sec: FsImpl::new(vault.clone(), &plugin, SensitivityClass::Secret),
            sqlite_pub: std::sync::OnceLock::new(),
            sqlite_priv: std::sync::OnceLock::new(),
            sqlite_sec: std::sync::OnceLock::new(),
            plugin,
            vault,
            bus,
        }
    }
}

impl Host for RealHost {
    fn plugin_name(&self) -> &str {
        &self.plugin
    }
    fn bus(&self) -> &dyn Bus {
        self.bus.as_ref()
    }
    fn vault(&self) -> &dyn Vault {
        self.vault.as_ref()
    }

    fn kv(&self, class: SensitivityClass) -> ApiResult<&dyn Kv> {
        Ok(match class {
            SensitivityClass::Public => &self.kv_pub,
            SensitivityClass::Private => &self.kv_priv,
            SensitivityClass::Secret => &self.kv_sec,
        })
    }
    fn fs(&self, class: SensitivityClass) -> ApiResult<&dyn Fs> {
        Ok(match class {
            SensitivityClass::Public => &self.fs_pub,
            SensitivityClass::Private => &self.fs_priv,
            SensitivityClass::Secret => &self.fs_sec,
        })
    }
    fn sqlite(&self, schema_sql: &str, class: SensitivityClass) -> ApiResult<&dyn Sqlite> {
        let cell = match class {
            SensitivityClass::Public => &self.sqlite_pub,
            SensitivityClass::Private => &self.sqlite_priv,
            SensitivityClass::Secret => &self.sqlite_sec,
        };
        if let Some(s) = cell.get() {
            return Ok(s);
        }
        let s = SqliteImpl::open(self.vault.clone(), &self.plugin, class, schema_sql)
            .map_err(|e| Error::Vault(e.to_string()))?;
        let _ = cell.set(s); // race: another thread may have populated; either is fine
        Ok(cell.get().unwrap())
    }
}

#[cfg(test)]
mod tests_realhost {
    use super::*;
    use crate::host::vault::store::{MemStore, Store};

    #[test]
    fn realhost_implements_host() {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [4u8; 32]);
        v.unseal_full();
        let h = RealHost::new("demo", Arc::new(v), Arc::new(InProcBus::new()));
        assert_eq!(h.plugin_name(), "demo");
        // Smoke: kv(Public) returns a working handle
        h.kv(SensitivityClass::Public)
            .unwrap()
            .put("k", serde_json::json!(1))
            .unwrap();
    }
}
