//! In-memory host for plugin unit tests. Enable via `--features test-host`.
use crate::{
    bus::{Bus, Event, Reply, Request},
    handles::{Fs, Kv, Sqlite},
    host::Host,
    manifest::SensitivityClass,
    vault::{SealState, Vault},
    Error, Result,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct TestHost {
    plugin: String,
    bus: TestBus,
    vault: TestVault,
    kv_pub: TestKv,
    kv_priv: TestKv,
    kv_sec: TestKv,
    sqlite_pub: TestSqlite,
    sqlite_priv: TestSqlite,
    sqlite_sec: TestSqlite,
    fs_pub: TestFs,
    fs_priv: TestFs,
    fs_sec: TestFs,
}

impl TestHost {
    pub fn new(plugin: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            bus: TestBus::default(),
            vault: TestVault::new_unsealed(),
            kv_pub: TestKv::default(),
            kv_priv: TestKv::default(),
            kv_sec: TestKv::default(),
            sqlite_pub: TestSqlite::default(),
            sqlite_priv: TestSqlite::default(),
            sqlite_sec: TestSqlite::default(),
            fs_pub: TestFs::default(),
            fs_priv: TestFs::default(),
            fs_sec: TestFs::default(),
        }
    }

    /// Return all (sql, params) pairs recorded against the public sqlite handle.
    pub fn sqlite_pub_calls(&self) -> Vec<(String, Vec<serde_json::Value>)> {
        self.sqlite_pub.calls()
    }
}

impl Host for TestHost {
    fn plugin_name(&self) -> &str {
        &self.plugin
    }
    fn bus(&self) -> &dyn Bus {
        &self.bus
    }
    fn vault(&self) -> &dyn Vault {
        &self.vault
    }
    fn kv(&self, class: SensitivityClass) -> Result<&dyn Kv> {
        Ok(match class {
            SensitivityClass::Public => &self.kv_pub,
            SensitivityClass::Private => &self.kv_priv,
            SensitivityClass::Secret => &self.kv_sec,
        })
    }
    fn sqlite(&self, _schema: &str, class: SensitivityClass) -> Result<&dyn Sqlite> {
        Ok(match class {
            SensitivityClass::Public => &self.sqlite_pub,
            SensitivityClass::Private => &self.sqlite_priv,
            SensitivityClass::Secret => &self.sqlite_sec,
        })
    }
    fn fs(&self, class: SensitivityClass) -> Result<&dyn Fs> {
        Ok(match class {
            SensitivityClass::Public => &self.fs_pub,
            SensitivityClass::Private => &self.fs_priv,
            SensitivityClass::Secret => &self.fs_sec,
        })
    }
}

#[derive(Default)]
pub struct TestBus {
    handlers: Mutex<HashMap<(String, String), Box<dyn Fn(Request) -> Result<Reply> + Send + Sync>>>,
    events: Mutex<Vec<Event>>,
}

impl TestBus {
    pub fn register<F>(&self, target: impl Into<String>, method: impl Into<String>, f: F)
    where
        F: Fn(Request) -> Result<Reply> + Send + Sync + 'static,
    {
        self.handlers
            .lock()
            .unwrap()
            .insert((target.into(), method.into()), Box::new(f));
    }
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

impl Bus for TestBus {
    fn request(&self, req: Request) -> Result<Reply> {
        let key = (req.target.clone(), req.method.clone());
        let handlers = self.handlers.lock().unwrap();
        match handlers.get(&key) {
            Some(h) => h(req),
            None => Err(Error::UnknownMethod(format!(
                "{}::{}",
                req.target, req.method
            ))),
        }
    }
    fn emit(&self, evt: Event) -> Result<()> {
        self.events.lock().unwrap().push(evt);
        Ok(())
    }
}

pub struct TestVault {
    state: Mutex<SealState>,
    store: Mutex<HashMap<String, Vec<u8>>>,
}

impl TestVault {
    pub fn new_unsealed() -> Self {
        Self {
            state: Mutex::new(SealState::FullyUnsealed),
            store: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for TestVault {
    fn default() -> Self {
        Self::new_unsealed()
    }
}

impl Vault for TestVault {
    fn state(&self) -> SealState {
        *self.state.lock().unwrap()
    }
    fn read_secret(&self, key: &str) -> Result<Vec<u8>> {
        if *self.state.lock().unwrap() != SealState::FullyUnsealed {
            return Err(Error::Sealed);
        }
        self.store
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or_else(|| Error::Vault(format!("missing key {key}")))
    }
    fn write_secret(&self, key: &str, val: &[u8]) -> Result<()> {
        if *self.state.lock().unwrap() != SealState::FullyUnsealed {
            return Err(Error::Sealed);
        }
        self.store.lock().unwrap().insert(key.into(), val.to_vec());
        Ok(())
    }
}

#[derive(Default)]
pub struct TestKv {
    store: Mutex<HashMap<String, Value>>,
}

impl Kv for TestKv {
    fn get(&self, key: &str) -> Result<Option<Value>> {
        Ok(self.store.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, val: Value) -> Result<()> {
        self.store.lock().unwrap().insert(key.into(), val);
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.store.lock().unwrap().remove(key);
        Ok(())
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

#[derive(Default)]
pub struct TestSqlite {
    calls: Mutex<Vec<(String, Vec<Value>)>>,
}

impl Sqlite for TestSqlite {
    fn execute(&self, sql: &str, params: &[Value]) -> Result<u64> {
        self.calls
            .lock()
            .unwrap()
            .push((sql.into(), params.to_vec()));
        Ok(0)
    }
    fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>> {
        self.calls
            .lock()
            .unwrap()
            .push((sql.into(), params.to_vec()));
        Ok(vec![])
    }
}

impl TestSqlite {
    pub fn calls(&self) -> Vec<(String, Vec<Value>)> {
        self.calls.lock().unwrap().clone()
    }
}

#[derive(Default)]
pub struct TestFs {
    store: Mutex<HashMap<String, Vec<u8>>>,
}

impl Fs for TestFs {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.store
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| Error::Vault(format!("no fs {path}")))
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        self.store
            .lock()
            .unwrap()
            .insert(path.into(), data.to_vec());
        Ok(())
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

#[cfg(all(test, feature = "test-host"))]
mod tests {
    use super::*;
    use crate::host::Host;
    use crate::manifest::SensitivityClass;

    #[test]
    fn test_host_kv_round_trip() {
        let h = TestHost::new("p");
        let kv = h.kv(SensitivityClass::Public).unwrap();
        kv.put("k", serde_json::json!(1)).unwrap();
        assert_eq!(kv.get("k").unwrap(), Some(serde_json::json!(1)));
    }
}
