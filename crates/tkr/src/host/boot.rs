use std::sync::{Arc, OnceLock};
use anyhow::Result;

static HOST: OnceLock<HostHandle> = OnceLock::new();

/// Returns a reference to the process-global `HostHandle`.
/// Panics if `boot()` has not been called yet.
pub fn get_host() -> &'static HostHandle {
    HOST.get().expect("HostHandle not initialized — call host::boot::boot() first")
}

/// Initialize the process-global `HostHandle`. Call once at startup.
/// Returns a reference to the handle so callers can verify it booted.
pub fn init() -> Result<&'static HostHandle> {
    let h = boot()?;
    HOST.set(h).ok();
    Ok(get_host())
}

use crate::host::{
    bus::InProcBus,
    loader::PluginRegistry,
    vault::{
        HostVault,
        store::{FsStore, MemStore, Store},
    },
};
use tkr_api::capability::{CapSet, STDOUT_FILTER, VAULT_READ_PUBLIC, VAULT_WRITE_PUBLIC};

/// Process-global host resources.
pub struct HostHandle {
    pub registry: Arc<PluginRegistry>,
    pub vault: Arc<HostVault>,
    pub bus: Arc<InProcBus>,
}

/// Bootstrap the host: vault → bus → registry → plugins loaded and started.
///
/// Falls back to an in-memory vault if the OS keychain is unavailable (CI/headless).
pub fn boot() -> Result<HostHandle> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let vault_root = home.join(".tkr").join("vault");
    std::fs::create_dir_all(&vault_root)?;

    let vault_root_str = vault_root.to_string_lossy().into_owned();

    // Try to get/create the master key from the OS keychain.
    let (store, master): (Arc<dyn Store>, [u8; 32]) =
        match crate::host::vault::keychain::init_master_key_if_missing("tkr-vault", &vault_root_str) {
            Ok(key_bytes) => {
                let mut master = [0u8; 32];
                let len = key_bytes.len().min(32);
                master[..len].copy_from_slice(&key_bytes[..len]);
                let store: Arc<dyn Store> = Arc::new(
                    FsStore::new(&vault_root)
                        .unwrap_or_else(|_| panic!("cannot open vault dir {}", vault_root.display())),
                );
                (store, master)
            }
            Err(e) => {
                eprintln!("tkr: keychain unavailable ({e}); using in-memory vault (no persistence)");
                let store: Arc<dyn Store> = Arc::new(MemStore::default());
                (store, [0u8; 32])
            }
        };

    let vault = Arc::new(HostVault::new(store, master));
    // auto-unseal on boot: public-class storage is available immediately.
    // (Full unseal — private/secret — requires explicit `tkr vault unseal`.)

    let bus = Arc::new(InProcBus::new());
    let mut registry = PluginRegistry::new(vault.clone(), bus.clone());

    // Grant capabilities to built-in plugins.
    let mut filter_caps = CapSet::new();
    filter_caps.grant(STDOUT_FILTER);
    registry.grant("tkr-filter", filter_caps);

    let mut analytics_caps = CapSet::new();
    analytics_caps.grant(STDOUT_FILTER);
    analytics_caps.grant(VAULT_READ_PUBLIC);
    analytics_caps.grant(VAULT_WRITE_PUBLIC);
    registry.grant("tkr-analytics", analytics_caps);

    // Register built-in filter plugin — loads bundled TOML rules first, then
    // user overrides from ~/.tkr/filters/.
    {
        use tkr_filter::FilterPlugin;
        use tkr_filter::v2::FilterPluginV2;

        let mut inner = FilterPlugin::new();
        if let Some(bundled) = crate::config::bundled_filters_dir() {
            let _ = inner.load_dir(&bundled);
        }
        let user_dir = home.join(".tkr").join("filters");
        if user_dir.exists() {
            let _ = inner.load_dir(&user_dir);
        }
        registry.register(Box::new(FilterPluginV2::new(inner)))?;
    }

    // Register analytics plugin (v2 — vault-backed sqlite).
    {
        use tkr_analytics::AnalyticsPluginV2;
        registry.register(Box::new(AnalyticsPluginV2::new()))?;
    }

    // on_load: initialize schemas and run migrations.
    // Non-essential: log and continue if analytics on_load fails (vault may be
    // sealed for secret-class; analytics only uses public-class storage).
    if let Err(e) = registry.load_all() {
        eprintln!("tkr: plugin load warning: {e}");
    }

    // on_start: plugins can begin background work.
    registry.start_all();

    Ok(HostHandle {
        registry: Arc::new(registry),
        vault,
        bus,
    })
}
