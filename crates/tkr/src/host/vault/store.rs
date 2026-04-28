use std::collections::HashMap;
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
