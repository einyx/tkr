use crate::Result;
use serde_json::Value;

pub trait Kv: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Value>>;
    fn put(&self, key: &str, val: Value) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
    fn list(&self, prefix: &str) -> Result<Vec<String>>;
}

pub trait Sqlite: Send + Sync {
    fn execute(&self, sql: &str, params: &[Value]) -> Result<u64>;
    fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>>;
}

pub trait Fs: Send + Sync {
    fn read(&self, path: &str) -> Result<Vec<u8>>;
    fn write(&self, path: &str, data: &[u8]) -> Result<()>;
    fn list(&self, path: &str) -> Result<Vec<String>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _kv_object_safe(_: &dyn Kv) {}
    fn _sqlite_object_safe(_: &dyn Sqlite) {}
    fn _fs_object_safe(_: &dyn Fs) {}
}
