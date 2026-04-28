use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use anyhow::Result;

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

pub struct FsStore {
    root: PathBuf,
}

impl FsStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
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

    fn save_index(&self, idx: &std::collections::BTreeMap<String, String>) -> Result<()> {
        let tmp = self.root.join("index.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(idx)?)?;
        std::fs::rename(&tmp, self.index_path())?;
        Ok(())
    }
}

impl Store for FsStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(self.entry_path(key)) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn put(&self, key: &str, val: &[u8]) -> Result<()> {
        let p = self.entry_path(key);
        let tmp = p.with_extension("age.tmp");
        std::fs::write(&tmp, val)?;
        std::fs::rename(&tmp, &p)?;
        let mut idx = self.load_index()?;
        let hash = p.file_stem().unwrap().to_string_lossy().to_string();
        idx.insert(hash, key.to_string());
        self.save_index(&idx)?;
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let p = self.entry_path(key);
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
        let mut idx = self.load_index()?;
        let hash = p.file_stem().unwrap().to_string_lossy().to_string();
        idx.remove(&hash);
        self.save_index(&idx)?;
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let idx = self.load_index()?;
        Ok(idx.into_values().filter(|k| k.starts_with(prefix)).collect())
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
}
