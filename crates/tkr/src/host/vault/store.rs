use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

pub trait Store: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn put(&self, key: &str, val: &[u8]) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
    fn list(&self, prefix: &str) -> Result<Vec<String>>;
}

#[derive(Default)]
pub struct MemStore {
    inner: Mutex<HashMap<String, Vec<u8>>>,
}

impl Store for MemStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.inner.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, val: &[u8]) -> Result<()> {
        self.inner.lock().unwrap().insert(key.into(), val.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.inner.lock().unwrap().remove(key);
        Ok(())
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// Per-process monotonic counter for unique tmp filenames.
static OP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_tmp_suffix() -> String {
    let pid = std::process::id();
    let seq = OP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{pid}.{seq}")
}

pub struct FsStore {
    root: PathBuf,
    /// Serializes all index-mutating operations (put, delete, save_index).
    lock: Mutex<()>,
}

impl FsStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        let store = Self {
            root,
            lock: Mutex::new(()),
        };
        // Reconcile: drop index entries whose .age files are missing (crash safety net).
        store.reconcile()?;
        Ok(store)
    }

    fn entry_path(&self, key: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(key.as_bytes());
        let name = hex::encode(h.finalize());
        self.root.join(format!("{name}.age"))
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    fn load_index(&self) -> Result<std::collections::BTreeMap<String, String>> {
        match std::fs::read(self.index_path()) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Write index atomically with fsync.
    /// Caller must hold `self.lock`.
    fn save_index(&self, idx: &std::collections::BTreeMap<String, String>) -> Result<()> {
        let suffix = next_tmp_suffix();
        let tmp = self.root.join(format!("index.json.{suffix}.tmp"));
        let json = serde_json::to_vec(idx)?;
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, self.index_path())?;
        Ok(())
    }

    /// Drop index entries whose backing `.age` files no longer exist.
    fn reconcile(&self) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let mut idx = self.load_index()?;
        let before = idx.len();
        idx.retain(|hash, _| self.root.join(format!("{hash}.age")).exists());
        if idx.len() != before {
            self.save_index(&idx)?;
        }
        Ok(())
    }
}

impl Store for FsStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        // get/list are lockless: they read via atomic-rename-safe files.
        match std::fs::read(self.entry_path(key)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn put(&self, key: &str, val: &[u8]) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let p = self.entry_path(key);
        let suffix = next_tmp_suffix();
        let tmp = p.with_extension(format!("age.{suffix}.tmp"));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(val)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &p)?;
        let mut idx = self.load_index()?;
        let hash = p.file_stem().unwrap().to_string_lossy().to_string();
        idx.insert(hash, key.to_string());
        self.save_index(&idx)?;
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let p = self.entry_path(key);
        // Update index FIRST so a crash between index write and file removal
        // leaves no dangling reference (get returns None, list won't surface it).
        let mut idx = self.load_index()?;
        let hash = p.file_stem().unwrap().to_string_lossy().to_string();
        idx.remove(&hash);
        self.save_index(&idx)?;
        // Now remove the actual file — orphaned file is harmless.
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        // lockless: one atomic read of index.json
        let idx = self.load_index()?;
        Ok(idx
            .into_values()
            .filter(|k| k.starts_with(prefix))
            .collect())
    }
}

#[cfg(test)]
mod tests_fs {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fsstore_round_trip_and_list() {
        let d = tempdir().unwrap();
        let s = FsStore::new(d.path()).unwrap();
        s.put("public/a", b"1").unwrap();
        s.put("public/b", b"2").unwrap();
        assert_eq!(s.get("public/a").unwrap(), Some(b"1".to_vec()));
        let mut keys = s.list("public/").unwrap();
        keys.sort();
        assert_eq!(keys, vec!["public/a", "public/b"]);
        s.delete("public/a").unwrap();
        assert_eq!(s.get("public/a").unwrap(), None);
    }

    #[test]
    fn put_fsyncs_and_index_consistent() {
        // Smoke: put then immediately reopen; entry survives.
        let d = tempdir().unwrap();
        {
            let s = FsStore::new(d.path()).unwrap();
            s.put("x", b"hello").unwrap();
        }
        let s2 = FsStore::new(d.path()).unwrap();
        assert_eq!(s2.get("x").unwrap(), Some(b"hello".to_vec()));
    }

    #[test]
    fn delete_makes_get_and_list_consistent_immediately() {
        let d = tempdir().unwrap();
        let s = FsStore::new(d.path()).unwrap();
        s.put("a", b"1").unwrap();
        s.put("b", b"2").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.get("a").unwrap(), None);
        let listed = s.list("").unwrap();
        assert!(!listed.contains(&"a".to_string()));
        assert!(listed.contains(&"b".to_string()));
    }

    #[test]
    fn reconciles_orphan_index_entry_on_open() {
        let d = tempdir().unwrap();
        {
            let s = FsStore::new(d.path()).unwrap();
            s.put("x", b"hi").unwrap();
        }
        // Manually nuke the data file but leave the index claiming it exists.
        let entries = std::fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("age"))
            .collect::<Vec<_>>();
        for e in entries {
            std::fs::remove_file(e.path()).unwrap();
        }
        // Reopen and list — orphan should be reconciled away.
        let s2 = FsStore::new(d.path()).unwrap();
        let listed = s2.list("").unwrap();
        assert!(!listed.contains(&"x".to_string()));
    }
}
