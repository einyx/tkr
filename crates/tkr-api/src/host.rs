use crate::{
    handles::{Kv, Sqlite, Fs},
    bus::Bus,
    vault::Vault,
    manifest::SensitivityClass,
    Result,
};

pub trait Host: Send + Sync {
    fn plugin_name(&self) -> &str;
    fn bus(&self) -> &dyn Bus;
    fn vault(&self) -> &dyn Vault;
    fn kv(&self, class: SensitivityClass) -> Result<&dyn Kv>;
    fn sqlite(&self, schema_sql: &str, class: SensitivityClass) -> Result<&dyn Sqlite>;
    fn fs(&self, class: SensitivityClass) -> Result<&dyn Fs>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn _host_object_safe(_: &dyn Host) {}
}
