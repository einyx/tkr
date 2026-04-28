use std::sync::{Arc, Mutex};
use tkr_api::plugin::Plugin;
use tkr_api::capability::CapSet;
use tkr_api::Error;
use anyhow::Result;
use crate::host::{RealHost, bus::InProcBus, vault::HostVault};

pub struct PluginRegistry {
    vault: Arc<HostVault>,
    bus: Arc<InProcBus>,
    /// (plugin, host, manifest, degraded)
    entries: Vec<Entry>,
    /// Per-plugin capability grants from config (caller-side; bus enforcement uses these).
    grants: std::collections::HashMap<String, CapSet>,
}

struct Entry {
    name: String,
    plugin: Mutex<Box<dyn Plugin>>,
    host: Arc<RealHost>,
    degraded: std::sync::atomic::AtomicBool,
}

impl PluginRegistry {
    pub fn new(vault: Arc<HostVault>, bus: Arc<InProcBus>) -> Self {
        Self {
            vault,
            bus,
            entries: Vec::new(),
            grants: Default::default(),
        }
    }

    /// Provide a per-plugin capability grant set (typically read from `~/.tkr/config.toml`).
    pub fn grant(&mut self, plugin: impl Into<String>, caps: CapSet) {
        self.grants.insert(plugin.into(), caps);
    }

    /// Register a plugin instance. Does not call `on_load` yet; that happens in `load_all`.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) -> Result<()> {
        let m = plugin.manifest();
        let name = m.name.clone();
        let granted = self.grants.get(&name).cloned().unwrap_or_default();
        // Check that every required capability is granted.
        for cap in &m.capabilities_required {
            if !granted.holds(cap) {
                return Err(anyhow::anyhow!(Error::CapabilityDenied {
                    cap: cap.clone(),
                    plugin: name.clone(),
                }));
            }
        }
        self.bus.set_caps(&name, granted);
        let host = Arc::new(RealHost::new(&name, self.vault.clone(), self.bus.clone()));
        self.entries.push(Entry {
            name,
            plugin: Mutex::new(plugin),
            host,
            degraded: false.into(),
        });
        Ok(())
    }

    /// Run `on_load` for every registered plugin; abort startup on first error.
    pub fn load_all(&self) -> Result<()> {
        for e in &self.entries {
            let host: &dyn tkr_api::host::Host = e.host.as_ref();
            let mut p = e.plugin.lock().unwrap();
            p.on_load(host)
                .map_err(|err| anyhow::anyhow!("plugin {}: on_load: {}", e.name, err))?;
        }
        Ok(())
    }

    /// Run `on_start`; capture errors as degraded (don't abort).
    pub fn start_all(&self) {
        for e in &self.entries {
            let mut p = e.plugin.lock().unwrap();
            if let Err(err) = p.on_start() {
                eprintln!(
                    "plugin {}: on_start failed: {}, marking degraded",
                    e.name, err
                );
                e.degraded
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    pub fn shutdown_all(&self) {
        for e in &self.entries {
            let mut p = e.plugin.lock().unwrap();
            if let Err(err) = p.on_shutdown() {
                eprintln!(
                    "plugin {}: on_shutdown error (ignored): {}",
                    e.name, err
                );
            }
        }
    }

    pub fn is_degraded(&self, name: &str) -> bool {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.degraded.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::vault::store::{MemStore, Store};
    use std::sync::Arc;
    use tkr_api::manifest::Manifest;

    struct Stub {
        name: &'static str,
        order: Arc<Mutex<Vec<String>>>,
        fail_start: bool,
    }

    impl Plugin for Stub {
        fn manifest(&self) -> Manifest {
            Manifest {
                name: self.name.into(),
                version: "0".into(),
                ..Default::default()
            }
        }
        fn on_load(&mut self, _host: &dyn tkr_api::host::Host) -> tkr_api::Result<()> {
            self.order
                .lock()
                .unwrap()
                .push(format!("load:{}", self.name));
            Ok(())
        }
        fn on_start(&mut self) -> tkr_api::Result<()> {
            if self.fail_start {
                return Err(tkr_api::Error::Plugin("boom".into()));
            }
            self.order
                .lock()
                .unwrap()
                .push(format!("start:{}", self.name));
            Ok(())
        }
        fn on_shutdown(&mut self) -> tkr_api::Result<()> {
            self.order
                .lock()
                .unwrap()
                .push(format!("shutdown:{}", self.name));
            Ok(())
        }
    }

    fn fresh() -> (PluginRegistry, Arc<Mutex<Vec<String>>>) {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [9u8; 32]);
        v.unseal_full();
        let order = Arc::new(Mutex::new(vec![]));
        (
            PluginRegistry::new(Arc::new(v), Arc::new(InProcBus::new())),
            order,
        )
    }

    #[test]
    fn lifecycle_runs_load_then_start_then_shutdown() {
        let (mut reg, order) = fresh();
        reg.register(Box::new(Stub {
            name: "a",
            order: order.clone(),
            fail_start: false,
        }))
        .unwrap();
        reg.load_all().unwrap();
        reg.start_all();
        reg.shutdown_all();
        let observed = order.lock().unwrap().clone();
        assert_eq!(observed, vec!["load:a", "start:a", "shutdown:a"]);
    }

    #[test]
    fn capability_ungranted_aborts_register() {
        let (mut reg, _order) = fresh();
        struct PrivStub;
        impl Plugin for PrivStub {
            fn manifest(&self) -> Manifest {
                Manifest {
                    name: "p".into(),
                    version: "0".into(),
                    capabilities_required: vec!["cap:vault.read.secret".into()],
                    ..Default::default()
                }
            }
            fn on_load(
                &mut self,
                _h: &dyn tkr_api::host::Host,
            ) -> tkr_api::Result<()> {
                Ok(())
            }
        }
        let err = reg.register(Box::new(PrivStub)).unwrap_err();
        assert!(format!("{err}").contains("cap:vault.read.secret"));
    }

    #[test]
    fn start_failure_marks_degraded_and_continues() {
        let (mut reg, order) = fresh();
        reg.register(Box::new(Stub {
            name: "a",
            order: order.clone(),
            fail_start: true,
        }))
        .unwrap();
        reg.register(Box::new(Stub {
            name: "b",
            order: order.clone(),
            fail_start: false,
        }))
        .unwrap();
        reg.load_all().unwrap();
        reg.start_all();
        assert!(reg.is_degraded("a"));
        assert!(!reg.is_degraded("b"));
        let observed = order.lock().unwrap().clone();
        // Only "b" recorded a start because "a"'s on_start returned Err before pushing.
        assert!(observed.contains(&"start:b".to_string()));
        assert!(!observed.contains(&"start:a".to_string()));
    }
}
