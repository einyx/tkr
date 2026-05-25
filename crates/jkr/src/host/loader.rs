use crate::host::{bus::InProcBus, vault::HostVault, RealHost};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use jkr_api::capability::CapSet;
use jkr_api::plugin::Plugin;
use jkr_api::Error;

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
    plugin: Arc<Mutex<Box<dyn Plugin>>>,
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

    /// Provide a per-plugin capability grant set (typically read from `~/.jkr/config.toml`).
    pub fn grant(&mut self, plugin: impl Into<String>, caps: CapSet) {
        self.grants.insert(plugin.into(), caps);
    }

    /// Register a plugin instance. Does not call `on_load` yet; that happens in `load_all`.
    ///
    /// When the manifest declares non-empty `cli_subcommands`, registers a bus handler for
    /// `(plugin_name, "cli.invoke")` that forwards to `Plugin::on_request`.
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
        let plugin = Arc::new(Mutex::new(plugin));
        if !m.cli_subcommands.is_empty() {
            let invoke = plugin.clone();
            let target = name.clone();
            self.bus
                .register_handler(target, "cli.invoke", vec![], move |req| {
                    let mut p = invoke.lock().unwrap();
                    p.on_request(req)
                });
        }
        self.entries.push(Entry {
            name,
            plugin,
            host,
            degraded: false.into(),
        });
        Ok(())
    }

    /// Run `on_load` for every registered plugin; abort startup on first error.
    pub fn load_all(&self) -> Result<()> {
        for e in &self.entries {
            let host: std::sync::Arc<dyn jkr_api::host::Host> = e.host.clone();
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
                e.degraded.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    pub fn shutdown_all(&self) {
        for e in &self.entries {
            let mut p = e.plugin.lock().unwrap();
            if let Err(err) = p.on_shutdown() {
                eprintln!("plugin {}: on_shutdown error (ignored): {}", e.name, err);
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

    // ── Filter pipeline ──────────────────────────────────────────────────────

    /// Iterate filter-capable plugins (those whose manifest declares `cap:stdout.filter`)
    /// and call `on_command_begin` once per plugin before any lines flow.
    pub fn filters_command_begin(&self, ctx: &jkr_api::plugin::CommandCtx) {
        for e in self.filter_entries() {
            let mut p = e.plugin.lock().unwrap();
            if let Err(err) = p.on_command_begin(ctx) {
                eprintln!("plugin {}: on_command_begin: {}", e.name, err);
                e.degraded.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    /// Run a single line through the registered filter chain. Each plugin's `on_line`
    /// can transform / suppress the line; the resulting String (or None for suppress) is
    /// passed to the next plugin. Returns the final transformed line, or None if any
    /// plugin suppressed it.
    pub fn run_filters_line(
        &self,
        mut line: String,
        ctx: &jkr_api::plugin::CommandCtx,
    ) -> Option<String> {
        for e in self.filter_entries() {
            if e.degraded.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
            let mut p = e.plugin.lock().unwrap();
            match p.on_line(&line, ctx) {
                Ok(jkr_api::plugin::FilterDecision::Pass) => { /* keep line as-is */ }
                Ok(jkr_api::plugin::FilterDecision::Suppress)
                | Ok(jkr_api::plugin::FilterDecision::SuppressWithNote(_)) => return None,
                Ok(jkr_api::plugin::FilterDecision::Replace(s)) => {
                    line = s;
                }
                Ok(jkr_api::plugin::FilterDecision::Annotate(s)) => {
                    line.push_str(&s);
                }
                Err(err) => {
                    eprintln!("plugin {}: on_line: {}; marking degraded", e.name, err);
                    e.degraded.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        Some(line)
    }

    /// Call `on_command_end` for each filter plugin and concatenate their newline-delimited
    /// summaries. Returns the joined summary block (may be empty).
    pub fn filters_command_end(&self, ctx: &jkr_api::plugin::CommandCtx) -> String {
        let mut summary = String::new();
        for e in self.filter_entries() {
            let mut p = e.plugin.lock().unwrap();
            match p.on_command_end(ctx) {
                Ok(s) if !s.is_empty() => {
                    summary.push_str(&s);
                    if !s.ends_with('\n') {
                        summary.push('\n');
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    eprintln!("plugin {}: on_command_end: {}", e.name, err);
                    e.degraded.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        summary
    }

    fn filter_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| {
            let p = e.plugin.lock().unwrap();
            p.manifest()
                .capabilities_required
                .iter()
                .any(|c| c == jkr_api::capability::STDOUT_FILTER)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::vault::store::{MemStore, Store};
    use std::sync::Arc;
    use jkr_api::manifest::Manifest;

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
        fn on_load(
            &mut self,
            _host: std::sync::Arc<dyn jkr_api::host::Host>,
        ) -> jkr_api::Result<()> {
            self.order
                .lock()
                .unwrap()
                .push(format!("load:{}", self.name));
            Ok(())
        }
        fn on_start(&mut self) -> jkr_api::Result<()> {
            if self.fail_start {
                return Err(jkr_api::Error::Plugin("boom".into()));
            }
            self.order
                .lock()
                .unwrap()
                .push(format!("start:{}", self.name));
            Ok(())
        }
        fn on_shutdown(&mut self) -> jkr_api::Result<()> {
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
                _h: std::sync::Arc<dyn jkr_api::host::Host>,
            ) -> jkr_api::Result<()> {
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

#[cfg(test)]
mod tests_filters {
    use super::*;
    use crate::host::vault::store::{MemStore, Store};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use jkr_api::plugin::{CommandCtx, FilterDecision, Plugin};

    struct DropAll(Arc<AtomicUsize>);

    impl Plugin for DropAll {
        fn manifest(&self) -> jkr_api::manifest::Manifest {
            jkr_api::manifest::Manifest {
                name: "drop".into(),
                version: "0".into(),
                capabilities_required: vec![jkr_api::capability::STDOUT_FILTER.into()],
                ..Default::default()
            }
        }
        fn on_load(&mut self, _h: std::sync::Arc<dyn jkr_api::host::Host>) -> jkr_api::Result<()> {
            Ok(())
        }
        fn on_line(&mut self, _line: &str, _ctx: &CommandCtx) -> jkr_api::Result<FilterDecision> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(FilterDecision::Suppress)
        }
        fn on_command_end(&mut self, _ctx: &CommandCtx) -> jkr_api::Result<String> {
            Ok(format!("dropped {}\n", self.0.load(Ordering::Relaxed)))
        }
    }

    fn fresh_reg() -> (PluginRegistry, Arc<crate::host::vault::HostVault>) {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = crate::host::vault::HostVault::new(store, [9u8; 32]);
        v.unseal_full();
        let v = Arc::new(v);
        let reg = PluginRegistry::new(v.clone(), Arc::new(InProcBus::new()));
        (reg, v)
    }

    #[test]
    fn run_filters_suppress() {
        let (mut reg, _v) = fresh_reg();
        let count = Arc::new(AtomicUsize::new(0));
        let mut caps = jkr_api::capability::CapSet::default();
        caps.grant(jkr_api::capability::STDOUT_FILTER);
        reg.grant("drop", caps);
        reg.register(Box::new(DropAll(count.clone()))).unwrap();
        reg.load_all().unwrap();
        let ctx = CommandCtx {
            command: "git".into(),
            args: "status".into(),
            line_index: 0,
        };
        assert!(reg.run_filters_line("noisy".into(), &ctx).is_none());
        let summary = reg.filters_command_end(&ctx);
        assert!(summary.contains("dropped 1"));
    }
}
