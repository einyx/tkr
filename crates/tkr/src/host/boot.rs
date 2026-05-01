use anyhow::Result;
use std::sync::{Arc, Mutex, OnceLock};

static HOST: OnceLock<HostHandle> = OnceLock::new();
/// Tracks whether the full boot (vault + analytics) has been performed.
static FULL_BOOT_DONE: OnceLock<()> = OnceLock::new();
/// Guards the upgrade from fast→full boot so only one thread performs it.
static FULL_BOOT_MUTEX: Mutex<()> = Mutex::new(());

/// Returns a reference to the process-global `HostHandle`.
/// Panics if neither `ensure()` nor `ensure_full()` has been called yet.
pub fn get_host() -> &'static HostHandle {
    HOST.get()
        .expect("HostHandle not initialized — call host::boot::ensure() first")
}

/// Fast boot: register only the filter plugin. No keychain access, no vault,
/// no analytics. Sets HOST on first call and returns a reference.
///
/// The proxy hot path calls this so `tkr <cmd>` never touches the OS keychain.
pub fn ensure() -> Result<&'static HostHandle> {
    if HOST.get().is_none() {
        let h = boot_filter_only()?;
        HOST.set(h).ok();
    }
    Ok(get_host())
}

/// Full boot: open keychain + vault + register analytics plugin. Idempotent —
/// if `ensure()` was called first, supplements the existing handle without
/// rebuilding the filter registry.
///
/// Callers: `tkr gain`, `tkr suggest`, `tkr watch`, `tkr vault`, `tkr admin`.
pub fn ensure_full() -> Result<&'static HostHandle> {
    // Fast path: full boot already done.
    if FULL_BOOT_DONE.get().is_some() {
        return Ok(get_host());
    }

    // Ensure the fast boot has run first (idempotent).
    ensure()?;

    // Serialize the upgrade from fast→full.
    let _guard = FULL_BOOT_MUTEX.lock().unwrap();
    // Double-check after acquiring the lock.
    if FULL_BOOT_DONE.get().is_some() {
        return Ok(get_host());
    }

    // Perform the heavy portion: keychain + vault + analytics.
    let host = get_host();
    upgrade_to_full(host)?;
    FULL_BOOT_DONE.set(()).ok();
    Ok(host)
}

/// Lazy vault accessor: returns the vault, performing full boot on first call.
/// Safe to call from any thread. Used by the detached analytics-writer thread
/// in `proxy::run` so the main path stays fast.
pub fn vault() -> Arc<crate::host::vault::HostVault> {
    // Trigger full boot (idempotent). Ignore error — analytics is best-effort.
    let _ = ensure_full();
    get_host()
        .vault
        .lock()
        .unwrap()
        .clone()
        .expect("vault not initialized after ensure_full")
}

use crate::host::{
    bus::InProcBus,
    loader::PluginRegistry,
    vault::{
        store::{FsStore, MemStore, Store},
        HostVault,
    },
};
use tkr_api::capability::{
    CapSet, STDOUT_FILTER, VAULT_READ_PUBLIC, VAULT_READ_SECRET, VAULT_WRITE_PUBLIC,
    VAULT_WRITE_SECRET,
};

/// Process-global host resources.
///
/// `vault` starts as `None` after a fast boot and is populated by
/// `upgrade_to_full`. Use `host::boot::vault()` for lazy access.
pub struct HostHandle {
    pub registry: Arc<PluginRegistry>,
    /// None after fast boot; Some after `ensure_full()` / `upgrade_to_full()`.
    pub vault: Mutex<Option<Arc<HostVault>>>,
    pub bus: Arc<InProcBus>,
}

/// Fast-path boot: only the filter plugin is registered. No vault, no keychain.
/// Returns a `HostHandle` with `vault = None`.
fn boot_filter_only() -> Result<HostHandle> {
    // Use an in-memory vault for the filter plugin — it doesn't need storage.
    let mem_store: Arc<dyn Store> = Arc::new(MemStore::default());
    let fast_vault = Arc::new(HostVault::new(mem_store, [0u8; 32]));

    let bus = Arc::new(InProcBus::new());
    let mut registry = PluginRegistry::new(fast_vault.clone(), bus.clone());

    // Grant filter capability.
    let mut filter_caps = CapSet::new();
    filter_caps.grant(STDOUT_FILTER);
    registry.grant("tkr-filter", filter_caps);

    // Register the filter plugin with an empty set of rules.
    // Rules are loaded lazily per command via `filter_for_command()`.
    {
        use tkr_filter::v2::FilterPluginV2;
        use tkr_filter::FilterPlugin;
        let inner = FilterPlugin::new(); // empty — lazy loading fills it in
        registry.register(Box::new(FilterPluginV2::new(inner)))?;
    }

    // Run on_load + on_start for the filter plugin (both are no-ops).
    if let Err(e) = registry.load_all() {
        eprintln!("tkr: filter plugin load warning: {e}");
    }
    registry.start_all();

    Ok(HostHandle {
        registry: Arc::new(registry),
        vault: Mutex::new(None),
        bus,
    })
}

/// Upgrade an existing `HostHandle` with vault + analytics plugin.
/// Called once, under `FULL_BOOT_MUTEX`.
fn upgrade_to_full(host: &HostHandle) -> Result<()> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let vault_root = home.join(".tkr").join("vault");
    std::fs::create_dir_all(&vault_root)?;

    let vault_root_str = vault_root.to_string_lossy().into_owned();

    let (store, master): (Arc<dyn Store>, [u8; 32]) =
        match crate::host::vault::keychain::init_master_key_if_missing("tkr-vault", &vault_root_str)
        {
            Ok(key_bytes) => {
                let mut master = [0u8; 32];
                let len = key_bytes.len().min(32);
                master[..len].copy_from_slice(&key_bytes[..len]);
                let store: Arc<dyn Store> =
                    Arc::new(FsStore::new(&vault_root).unwrap_or_else(|_| {
                        panic!("cannot open vault dir {}", vault_root.display())
                    }));
                (store, master)
            }
            Err(e) => {
                eprintln!(
                    "tkr: could not initialize master key ({e}); using in-memory vault (no persistence)"
                );
                let store: Arc<dyn Store> = Arc::new(MemStore::default());
                (store, [0u8; 32])
            }
        };

    let full_vault = Arc::new(HostVault::new(store, master));

    // Store the real vault into the handle.
    *host.vault.lock().unwrap() = Some(full_vault.clone());

    // Register analytics plugin into the existing registry.
    // We need to build a new registry just for the analytics plugin
    // since PluginRegistry doesn't support adding plugins post-load_all.
    // Use a separate PluginRegistry for the analytics plugin; the analytics
    // thread uses `vault()` directly via `record_noise_signature_via_host`.
    {
        use tkr_analytics::AnalyticsPluginV2;
        let mut analytics_registry = PluginRegistry::new(full_vault.clone(), host.bus.clone());

        let mut analytics_caps = CapSet::new();
        analytics_caps.grant(STDOUT_FILTER);
        analytics_caps.grant(VAULT_READ_PUBLIC);
        analytics_caps.grant(VAULT_WRITE_PUBLIC);
        analytics_registry.grant("tkr-analytics", analytics_caps);

        analytics_registry.register(Box::new(AnalyticsPluginV2::new()))?;
        if let Err(e) = analytics_registry.load_all() {
            eprintln!("tkr: analytics plugin load warning: {e}");
        }
        analytics_registry.start_all();
        // analytics_registry is dropped here — the plugin itself is ephemeral
        // since the proxy path uses record_noise_signature_via_host directly.
    }

    // Register session-recorder plugin (v2 — vault-backed FS).
    {
        use tkr_session_recorder::SessionRecorderPluginV2;
        let mut recorder_registry = PluginRegistry::new(full_vault.clone(), host.bus.clone());

        let mut recorder_caps = CapSet::new();
        recorder_caps.grant(STDOUT_FILTER);
        recorder_caps.grant(VAULT_READ_SECRET);
        recorder_caps.grant(VAULT_WRITE_SECRET);
        recorder_registry.grant("tkr-session-recorder", recorder_caps);

        recorder_registry.register(Box::new(SessionRecorderPluginV2::new()))?;
        if let Err(e) = recorder_registry.load_all() {
            eprintln!("tkr: session-recorder plugin load warning: {e}");
        }
        recorder_registry.start_all();
    }

    Ok(())
}

// ── Per-command lazy filter cache ────────────────────────────────────────────

use std::collections::HashMap;
use tkr_filter::FilterPlugin;

/// Cache of per-command `FilterPlugin` instances, wrapped in `Mutex` to allow
/// `&mut self` access during `filter()` calls (rules like CollapseRepeats track
/// per-line state).
static FILTER_CACHE: OnceLock<Mutex<HashMap<String, Arc<Mutex<FilterPlugin>>>>> = OnceLock::new();

fn filter_cache() -> &'static Mutex<HashMap<String, Arc<Mutex<FilterPlugin>>>> {
    FILTER_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Return (or build and cache) a `FilterPlugin` loaded with only the rules
/// for `cmd_name`. Loads `<cmd_name>.toml` from the bundled filters dir and
/// `~/.tkr/filters/<cmd_name>.toml` from the user dir.
///
/// Falls back to an empty `FilterPlugin` if no matching TOML is found.
/// The returned `Arc<Mutex<FilterPlugin>>` can be locked for mutable access
/// during command processing.
pub fn filter_for_command(cmd_name: &str) -> Arc<Mutex<FilterPlugin>> {
    // Normalise: strip path prefix (e.g. "/usr/bin/git" → "git").
    let base = std::path::Path::new(cmd_name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cmd_name);

    {
        let cache = filter_cache().lock().unwrap();
        if let Some(fp) = cache.get(base) {
            return fp.clone();
        }
    }

    // Build the per-command filter plugin.
    let mut inner = FilterPlugin::new();
    let file_name = format!("{base}.toml");

    if let Some(bundled) = crate::config::bundled_filters_dir() {
        let toml_path = bundled.join(&file_name);
        if toml_path.exists() {
            let _ = inner.load_file(&toml_path);
        }
        // git-diff.toml, git-log.toml etc. are handled by the main `git.toml`
        // via the `subcommands` TOML selector.
    }

    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let user_toml = home.join(".tkr").join("filters").join(&file_name);
    if user_toml.exists() {
        let _ = inner.load_file(&user_toml);
    }

    let fp = Arc::new(Mutex::new(inner));
    filter_cache()
        .lock()
        .unwrap()
        .insert(base.to_string(), fp.clone());
    fp
}

/// Bootstrap the host: vault → bus → registry → plugins loaded and started.
///
/// Falls back to an in-memory vault if the master key cannot be loaded (e.g. CI/headless).
///
/// DEPRECATED: prefer `ensure()` (fast) or `ensure_full()` (full boot).
/// Kept for backwards compatibility with tests/benchmarks that call `boot()` directly.
pub fn boot() -> Result<HostHandle> {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let vault_root = home.join(".tkr").join("vault");
    std::fs::create_dir_all(&vault_root)?;

    let vault_root_str = vault_root.to_string_lossy().into_owned();

    // Try to create or load the master key (file under ~/.tkr/vault/, with legacy keychain migration).
    let (store, master): (Arc<dyn Store>, [u8; 32]) =
        match crate::host::vault::keychain::init_master_key_if_missing("tkr-vault", &vault_root_str)
        {
            Ok(key_bytes) => {
                let mut master = [0u8; 32];
                let len = key_bytes.len().min(32);
                master[..len].copy_from_slice(&key_bytes[..len]);
                let store: Arc<dyn Store> =
                    Arc::new(FsStore::new(&vault_root).unwrap_or_else(|_| {
                        panic!("cannot open vault dir {}", vault_root.display())
                    }));
                (store, master)
            }
            Err(e) => {
                eprintln!(
                    "tkr: could not initialize master key ({e}); using in-memory vault (no persistence)"
                );
                let store: Arc<dyn Store> = Arc::new(MemStore::default());
                (store, [0u8; 32])
            }
        };

    let vault = Arc::new(HostVault::new(store, master));

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

    let mut recorder_caps = CapSet::new();
    recorder_caps.grant(STDOUT_FILTER);
    recorder_caps.grant(VAULT_READ_SECRET);
    recorder_caps.grant(VAULT_WRITE_SECRET);
    registry.grant("tkr-session-recorder", recorder_caps);

    // Register built-in filter plugin — loads bundled TOML rules first, then
    // user overrides from ~/.tkr/filters/.
    {
        use tkr_filter::v2::FilterPluginV2;
        use tkr_filter::FilterPlugin;

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

    // Register session-recorder plugin (v2 — vault-backed FS).
    {
        use tkr_session_recorder::SessionRecorderPluginV2;
        registry.register(Box::new(SessionRecorderPluginV2::new()))?;
    }

    // on_load: initialize schemas and run migrations.
    if let Err(e) = registry.load_all() {
        eprintln!("tkr: plugin load warning: {e}");
    }

    // on_start: plugins can begin background work.
    registry.start_all();

    Ok(HostHandle {
        registry: Arc::new(registry),
        vault: Mutex::new(Some(vault)),
        bus,
    })
}

/// Legacy `init()` — calls full boot and stores in HOST.
/// Used only by tests/benchmarks.
pub fn init() -> Result<&'static HostHandle> {
    let h = boot()?;
    HOST.set(h).ok();
    FULL_BOOT_DONE.set(()).ok();
    Ok(get_host())
}
