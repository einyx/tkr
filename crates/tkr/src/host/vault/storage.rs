use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

use rusqlite::{params_from_iter, types::Value as SqlValue, Connection};
use serde_json::Value;
use tkr_api::manifest::SensitivityClass;
use tkr_api::{Error, Result};

use crate::host::vault::HostVault;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Register the sqlite-vec extension once at process start so every vault
/// sqlite connection gets the `vec0` virtual-table module. Required for
/// embedding-similarity tables.
fn ensure_sqlite_vec_loaded() {
    use std::sync::Once;
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

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

// ─── SqliteImpl ───────────────────────────────────────────────────────────────

pub struct SqliteImpl {
    vault: Arc<HostVault>,
    plugin: String,
    class: SensitivityClass,
    tmp_dir: PathBuf,
    tmp_path: PathBuf,
    conn: Mutex<Connection>,
}

impl SqliteImpl {
    pub fn open(
        vault: Arc<HostVault>,
        plugin: impl Into<String>,
        class: SensitivityClass,
        schema_sql: &str,
    ) -> Result<Self> {
        ensure_sqlite_vec_loaded();
        let plugin = plugin.into();
        let key = blob_key(&plugin, class);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_dir = std::env::temp_dir()
            .join(format!("tkr-sqlite-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&tmp_dir)
            .map_err(|e| Error::Vault(e.to_string()))?;
        let tmp_path = tmp_dir.join("db.sqlite");

        if let Some(z) = vault
            .read(class, &key, &plugin)
            .map_err(|e| Error::Vault(e.to_string()))?
        {
            std::fs::write(&tmp_path, &z[..])
                .map_err(|e| Error::Vault(e.to_string()))?;
        }

        let conn =
            Connection::open(&tmp_path).map_err(|e| Error::Vault(e.to_string()))?;
        conn.execute_batch(schema_sql)
            .map_err(|e| Error::Vault(e.to_string()))?;

        Ok(Self {
            vault,
            plugin,
            class,
            tmp_dir,
            tmp_path,
            conn: Mutex::new(conn),
        })
    }

    fn persist(&self) -> Result<()> {
        let bytes = std::fs::read(&self.tmp_path)
            .map_err(|e| Error::Vault(e.to_string()))?;
        let key = blob_key(&self.plugin, self.class);
        self.vault
            .write(self.class, &key, &bytes, &self.plugin)
            .map_err(|e| Error::Vault(e.to_string()))
    }
}

fn blob_key(plugin: &str, class: SensitivityClass) -> String {
    let cls = match class {
        SensitivityClass::Public => "public",
        SensitivityClass::Private => "private",
        SensitivityClass::Secret => "secret",
    };
    format!("sqlite/{plugin}/{cls}.db")
}

fn json_to_sql(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                SqlValue::Real(f)
            } else {
                SqlValue::Null
            }
        }
        Value::String(s) => SqlValue::Text(s.clone()),
        _ => SqlValue::Text(v.to_string()),
    }
}

fn sql_ref_to_json(v: &rusqlite::types::ValueRef) -> Value {
    use rusqlite::types::ValueRef::*;
    match v {
        Null => Value::Null,
        Integer(i) => Value::Number((*i).into()),
        Real(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        Blob(b) => Value::String(hex::encode(b)),
    }
}

impl tkr_api::handles::Sqlite for SqliteImpl {
    fn execute(&self, sql: &str, params: &[Value]) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let p: Vec<SqlValue> = params.iter().map(json_to_sql).collect();
        let n = conn
            .execute(sql, params_from_iter(p.iter()))
            .map_err(|e| Error::Vault(e.to_string()))?;
        drop(conn);
        self.persist()?;
        Ok(n as u64)
    }

    fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>> {
        let conn = self.conn.lock().unwrap();
        let p: Vec<SqlValue> = params.iter().map(json_to_sql).collect();
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| Error::Vault(e.to_string()))?;
        let col_count = stmt.column_count();
        let rows: Vec<Vec<Value>> = stmt
            .query_map(params_from_iter(p.iter()), |r| {
                let mut out = Vec::with_capacity(col_count);
                for i in 0..col_count {
                    out.push(sql_ref_to_json(&r.get_ref_unwrap(i)));
                }
                Ok(out)
            })
            .map_err(|e| Error::Vault(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
}

impl Drop for SqliteImpl {
    fn drop(&mut self) {
        let _ = self.persist();
        let _ = std::fs::remove_dir_all(&self.tmp_dir);
    }
}

#[cfg(test)]
mod tests_sqlite {
    use super::*;
    use crate::host::vault::store::{FsStore, Store};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tkr_api::handles::Sqlite as SqliteTrait;

    #[test]
    fn sqlite_persists_across_handle_drops() {
        let d = tempdir().unwrap();
        let master = [3u8; 32];
        let vault = {
            let store: Arc<dyn Store> = Arc::new(FsStore::new(d.path()).unwrap());
            let v = HostVault::new(store, master);
            v.unseal_full();
            Arc::new(v)
        };
        {
            let s = SqliteImpl::open(
                vault.clone(),
                "p",
                SensitivityClass::Public,
                "CREATE TABLE IF NOT EXISTS t(x INTEGER);",
            )
            .unwrap();
            s.execute(
                "INSERT INTO t VALUES (?)",
                &[Value::Number(42.into())],
            )
            .unwrap();
        }
        let s2 = SqliteImpl::open(
            vault.clone(),
            "p",
            SensitivityClass::Public,
            "CREATE TABLE IF NOT EXISTS t(x INTEGER);",
        )
        .unwrap();
        let rows = s2.query("SELECT x FROM t", &[]).unwrap();
        assert_eq!(rows[0][0], Value::Number(42.into()));
    }
}

// ─── FsImpl ──────────────────────────────────────────────────────────────────

pub struct FsImpl {
    vault: Arc<HostVault>,
    plugin: String,
    class: SensitivityClass,
}

impl FsImpl {
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

    fn key(&self, p: &str) -> String {
        format!("fs/{}/{}", self.plugin, p)
    }
}

impl tkr_api::handles::Fs for FsImpl {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let z = self
            .vault
            .read(self.class, &self.key(path), &self.plugin)
            .map_err(|e| Error::Vault(e.to_string()))?
            .ok_or_else(|| Error::Vault(format!("no fs entry {path}")))?;
        // Callers receiving plaintext are responsible for zeroizing on their side;
        // intermediate copies inside the vault are wiped via Zeroizing drop.
        Ok((*z).clone())
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        self.vault
            .write(self.class, &self.key(path), data, &self.plugin)
            .map_err(|e| Error::Vault(e.to_string()))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let full = format!("fs/{}/{}", self.plugin, prefix);
        let raw = self
            .vault
            .list(self.class, &full)
            .map_err(|e| Error::Vault(e.to_string()))?;
        let strip = format!("fs/{}/", self.plugin);
        Ok(raw
            .into_iter()
            .filter_map(|k| k.strip_prefix(&strip).map(|s| s.to_string()))
            .collect())
    }
}

#[cfg(test)]
mod tests_fs {
    use super::*;
    use crate::host::vault::store::{MemStore, Store};
    use std::sync::Arc;
    use tkr_api::handles::Fs;

    fn vault() -> Arc<HostVault> {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [9u8; 32]);
        v.unseal_full();
        Arc::new(v)
    }

    #[test]
    fn fs_round_trip() {
        let fs = FsImpl::new(vault(), "demo", SensitivityClass::Private);
        fs.write("a/b", b"data").unwrap();
        assert_eq!(fs.read("a/b").unwrap(), b"data".to_vec());
    }

    #[test]
    fn fs_namespaced_per_plugin() {
        let v = vault();
        let a = FsImpl::new(v.clone(), "alpha", SensitivityClass::Private);
        let b = FsImpl::new(v.clone(), "beta", SensitivityClass::Private);
        a.write("p", b"data").unwrap();
        assert!(b.read("p").is_err());
    }
}
