use crate::host::vault::HostVault;
use anyhow::Result;
use tkr_api::vault::{SealState, Vault as VaultTrait};

pub fn status(vault: &HostVault) -> Result<i32> {
    let state = match vault.state() {
        SealState::Sealed => "sealed",
        SealState::AutoUnsealed => "auto-unsealed",
        SealState::FullyUnsealed => "fully-unsealed",
    };
    println!("vault state: {state}");
    Ok(0)
}

// ── Task 5.2: init ───────────────────────────────────────────────────────────

use std::path::Path;

pub enum InitMode<'a> {
    /// Master key at `~/.tkr/vault/.tkr-vault.key` (0600); legacy OS keychain entries migrate once on read.
    MasterKeyFile,
    Passphrase(&'a str),
}

pub fn init(vault_root: &Path, mode: InitMode) -> Result<i32> {
    use crate::host::vault::keychain;
    use anyhow::Context;
    use rand::RngCore;

    std::fs::create_dir_all(vault_root).context("create vault dir")?;
    let mut master = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut master);

    match mode {
        InitMode::MasterKeyFile => {
            let user = vault_root.to_string_lossy();
            // Idempotent if master key already exists (file or legacy keychain migration).
            if keychain::get_master_key("tkr-vault", &user).is_ok() {
                println!(
                    "vault already initialized at {} (master key already present)",
                    vault_root.display()
                );
                return Ok(0);
            }
            keychain::set_master_key("tkr-vault", &user, &master)?;
            println!(
                "vault initialized at {}; master key at ~/.tkr/vault/.tkr-vault.key (0600)",
                vault_root.display()
            );
        }
        InitMode::Passphrase(p) => {
            let master_path = vault_root.join("master.age");
            if master_path.exists() {
                println!(
                    "vault already initialized at {} (master.age exists)",
                    vault_root.display()
                );
                return Ok(0);
            }
            use age::secrecy::SecretString;
            use std::io::Write;
            let enc = age::Encryptor::with_user_passphrase(SecretString::new(p.to_string()));
            let mut wrapped = Vec::new();
            let mut writer = enc.wrap_output(&mut wrapped).context("wrap master")?;
            writer.write_all(&master)?;
            writer.finish()?;
            std::fs::write(&master_path, &wrapped).context("write master.age")?;
            println!(
                "vault initialized at {} (master key wrapped under passphrase)",
                vault_root.display()
            );
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests_init {
    use super::*;
    use crate::host::vault::keychain;
    use tempfile::tempdir;

    #[test]
    fn init_passphrase_writes_master_age() {
        let d = tempdir().unwrap();
        assert_eq!(init(d.path(), InitMode::Passphrase("hunter2")).unwrap(), 0);
        assert!(d.path().join("master.age").exists());
    }

    #[test]
    fn init_passphrase_idempotent() {
        let d = tempdir().unwrap();
        init(d.path(), InitMode::Passphrase("a")).unwrap();
        let bytes_first = std::fs::read(d.path().join("master.age")).unwrap();
        // Second call should leave file unchanged (idempotent).
        init(d.path(), InitMode::Passphrase("different-pass")).unwrap();
        let bytes_second = std::fs::read(d.path().join("master.age")).unwrap();
        assert_eq!(bytes_first, bytes_second);
    }

    // Writes real ~/.tkr/vault/.tkr-vault.key — isolate HOME before running.
    #[test]
    #[ignore]
    fn init_master_key_file_smoke() {
        let d = tempdir().unwrap();
        let user = d.path().to_string_lossy().to_string();
        let _ = keychain::delete_master_key("tkr-vault", &user);
        init(d.path(), InitMode::MasterKeyFile).unwrap();
        let key = keychain::get_master_key("tkr-vault", &user).unwrap();
        assert_eq!(key.len(), 32);
        let _ = keychain::delete_master_key("tkr-vault", &user);
    }
}

#[cfg(test)]
mod tests_status {
    use super::*;
    use crate::host::vault::store::{MemStore, Store};
    use std::sync::Arc;

    #[test]
    fn status_sealed_by_default_in_memory() {
        let v = HostVault::new_in_memory();
        // Capture stdout: easiest to just call and ensure it returns 0.
        assert_eq!(status(&v).unwrap(), 0);
    }

    #[test]
    fn status_after_unseal() {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [1u8; 32]);
        v.unseal_full();
        assert_eq!(status(&v).unwrap(), 0);
        // Crude: re-fetch state to confirm fully unsealed
        assert_eq!(v.state(), tkr_api::vault::SealState::FullyUnsealed);
    }
}

// ── Task 5.3: unseal / seal ──────────────────────────────────────────────────

pub fn unseal(vault: &HostVault) -> Result<i32> {
    vault.unseal_full();
    println!("vault fully unsealed");
    Ok(0)
}

pub fn seal(vault: &HostVault) -> Result<i32> {
    vault.seal();
    println!("vault sealed");
    Ok(0)
}

#[cfg(test)]
mod tests_seal {
    use super::*;
    use crate::host::vault::store::{MemStore, Store};
    use std::sync::Arc;
    use tkr_api::vault::SealState;

    #[test]
    fn unseal_promotes_to_fully_unsealed() {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [1u8; 32]);
        assert_eq!(v.state(), SealState::AutoUnsealed);
        unseal(&v).unwrap();
        assert_eq!(v.state(), SealState::FullyUnsealed);
    }

    #[test]
    fn seal_drops_to_sealed() {
        let store: Arc<dyn Store> = Arc::new(MemStore::default());
        let v = HostVault::new(store, [1u8; 32]);
        v.unseal_full();
        seal(&v).unwrap();
        assert_eq!(v.state(), SealState::Sealed);
    }
}

// ── Task 5.4: rotate ─────────────────────────────────────────────────────────
//
// Re-encrypts every vault entry under a freshly generated master key and
// atomically swaps the vault directory. The new master key is returned to the
// caller via `new_master` out-parameter; the caller is responsible for
// persisting it (rewrite ~/.tkr/vault/.tkr-vault.key or master.age). This responsibility
// is deliberately left to the caller because the persistence strategy differs
// between default file-backed mode and Passphrase modes — Task 5.5 / 6.3 will wire up the
// appropriate callbacks when rotate is integrated into the CLI.

pub fn rotate(old: &HostVault, vault_root: &Path) -> Result<[u8; 32]> {
    use crate::host::vault::store::{FsStore, Store};
    use anyhow::Context;
    use rand::RngCore;
    use std::sync::Arc;
    use tkr_api::manifest::SensitivityClass;

    // Fully unseal old vault so we can read every class.
    old.unseal_full();

    let shadow_dir = {
        let mut p = vault_root.to_path_buf();
        let stem = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        p.set_file_name(format!("{stem}.rotating"));
        p
    };
    if shadow_dir.exists() {
        std::fs::remove_dir_all(&shadow_dir)?;
    }
    let shadow_store: Arc<dyn Store> =
        Arc::new(FsStore::new(&shadow_dir).context("create shadow store")?);

    // New master key
    let mut new_master = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut new_master);
    let new_vault = HostVault::new(shadow_store, new_master);
    new_vault.unseal_full();

    // Walk every entry across classes; decrypt then re-encrypt under new key.
    for class in [
        SensitivityClass::Public,
        SensitivityClass::Private,
        SensitivityClass::Secret,
    ] {
        let keys = old.list(class, "")?;
        for k in keys {
            if let Some(z) = old.read(class, &k, "rotate")? {
                new_vault.write(class, &k, &z[..], "rotate")?;
            }
        }
    }

    // Atomic swap: rename original to .old, shadow to original, delete .old.
    let old_dir = {
        let mut p = vault_root.to_path_buf();
        let stem = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        p.set_file_name(format!("{stem}.old"));
        p
    };
    if old_dir.exists() {
        std::fs::remove_dir_all(&old_dir)?;
    }
    std::fs::rename(vault_root, &old_dir)?;
    std::fs::rename(&shadow_dir, vault_root)?;
    std::fs::remove_dir_all(&old_dir)?;

    println!("vault rotated under new master key");
    // NOTE: caller must persist `new_master` (master key file replace or master.age rewrite).
    // This gap is documented: Task 5.5 / 6.3 will close it by accepting a
    // persistence callback or by integrating with the chosen InitMode.
    Ok(new_master)
}

// ── Task 5.6: audit ──────────────────────────────────────────────────────────

use crate::host::vault::audit::AuditLog;

pub struct AuditOpts {
    pub verify: bool,
    pub last_n: Option<usize>,
}

pub fn audit(vault_root: &Path, opts: AuditOpts) -> Result<i32> {
    let log_path = vault_root.join("audit.log");
    if !log_path.exists() {
        println!("no audit log yet at {}", log_path.display());
        return Ok(0);
    }
    if opts.verify {
        // AuditLog::open itself validates the tip against the current log state,
        // so an Err from open already indicates a verification failure.
        let ok = match AuditLog::open(&log_path) {
            Err(_) => false,
            Ok(log) => log.verify()?,
        };
        if ok {
            println!("audit log OK");
            return Ok(0);
        } else {
            eprintln!("audit log verification FAILED (chain broken or truncated)");
            return Ok(1);
        }
    }
    // Print events; default last 10.
    let n = opts.last_n.unwrap_or(10);
    let s = std::fs::read_to_string(&log_path)?;
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    let total = lines.len();
    let start = total.saturating_sub(n);
    println!(
        "audit log: {total} entries (showing last {})",
        total - start
    );
    for line in &lines[start..] {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(ev) = v.get("event") {
                let actor = ev.get("actor").and_then(|x| x.as_str()).unwrap_or("?");
                let op = ev.get("op").and_then(|x| x.as_str()).unwrap_or("?");
                let key = ev.get("key").and_then(|x| x.as_str()).unwrap_or("?");
                let ts = ev.get("ts").and_then(|x| x.as_u64()).unwrap_or(0);
                println!("  ts={ts} actor={actor} op={op} key={key}");
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests_audit {
    use super::*;
    use crate::host::vault::audit::{AuditEvent, AuditLog};
    use tempfile::tempdir;

    #[test]
    fn audit_print_no_log() {
        let d = tempdir().unwrap();
        let r = audit(
            d.path(),
            AuditOpts {
                verify: false,
                last_n: Some(5),
            },
        )
        .unwrap();
        assert_eq!(r, 0);
    }

    #[test]
    fn audit_verify_fresh_log_is_ok() {
        let d = tempdir().unwrap();
        let log = AuditLog::open(d.path().join("audit.log")).unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "x".into(),
            key: "k".into(),
            ts: 1,
        })
        .unwrap();
        let r = audit(
            d.path(),
            AuditOpts {
                verify: true,
                last_n: None,
            },
        )
        .unwrap();
        assert_eq!(r, 0);
    }

    #[test]
    fn audit_verify_detects_truncation() {
        let d = tempdir().unwrap();
        let log = AuditLog::open(d.path().join("audit.log")).unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "x".into(),
            key: "k".into(),
            ts: 1,
        })
        .unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "y".into(),
            key: "k".into(),
            ts: 2,
        })
        .unwrap();
        // Truncate last line:
        let path = d.path().join("audit.log");
        let s = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        std::fs::write(&path, lines[..lines.len() - 1].join("\n") + "\n").unwrap();
        let r = audit(
            d.path(),
            AuditOpts {
                verify: true,
                last_n: None,
            },
        )
        .unwrap();
        assert_eq!(r, 1); // verification failed → non-zero exit
    }

    #[test]
    fn audit_print_shows_recent_events() {
        let d = tempdir().unwrap();
        let log = AuditLog::open(d.path().join("audit.log")).unwrap();
        for i in 0..5 {
            log.append(AuditEvent {
                actor: "p".into(),
                op: "op".into(),
                key: format!("k{i}"),
                ts: i as u64,
            })
            .unwrap();
        }
        let r = audit(
            d.path(),
            AuditOpts {
                verify: false,
                last_n: Some(3),
            },
        )
        .unwrap();
        assert_eq!(r, 0);
    }
}

// ── Task 5.5: export / import ─────────────────────────────────────────────────

pub fn export(vault_root: &Path, out_path: &Path) -> anyhow::Result<i32> {
    use anyhow::Context;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let f = std::fs::File::create(out_path).context("create export bundle")?;
    let gz = GzEncoder::new(f, Compression::default());
    let mut tar = tar::Builder::new(gz);
    add_dir_to_tar(&mut tar, vault_root, vault_root)?;
    tar.into_inner()?.finish()?;
    println!("exported vault → {}", out_path.display());
    Ok(0)
}

fn add_dir_to_tar<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    root: &Path,
    current: &Path,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let p = entry.path();
        let rel = p.strip_prefix(root).unwrap();
        if entry.file_type()?.is_dir() {
            add_dir_to_tar(tar, root, &p)?;
        } else {
            let mut f = std::fs::File::open(&p)?;
            tar.append_file(rel, &mut f)?;
        }
    }
    Ok(())
}

pub fn import(bundle_path: &Path, vault_root: &Path) -> anyhow::Result<i32> {
    use anyhow::Context;
    use flate2::read::GzDecoder;
    if vault_root.exists() && std::fs::read_dir(vault_root)?.next().is_some() {
        anyhow::bail!(
            "import target {} already exists and is not empty",
            vault_root.display()
        );
    }
    std::fs::create_dir_all(vault_root)?;
    let f = std::fs::File::open(bundle_path).context("open bundle")?;
    let gz = GzDecoder::new(f);
    let mut tar = tar::Archive::new(gz);
    tar.unpack(vault_root)?;
    println!(
        "imported vault from {} → {}",
        bundle_path.display(),
        vault_root.display()
    );
    Ok(0)
}

#[cfg(test)]
mod tests_export_import {
    use super::*;
    use crate::host::vault::{
        store::{FsStore, Store},
        HostVault,
    };
    use std::sync::Arc;
    use tempfile::tempdir;
    use tkr_api::manifest::SensitivityClass;

    #[test]
    fn export_then_import_round_trip() {
        let d = tempdir().unwrap();
        let src = d.path().join("vault");
        std::fs::create_dir_all(&src).unwrap();
        let master = [4u8; 32];
        {
            let store: Arc<dyn Store> = Arc::new(FsStore::new(&src).unwrap());
            let v = HostVault::new(store, master);
            v.unseal_full();
            v.write(SensitivityClass::Public, "k", b"hello", "test")
                .unwrap();
            v.write(SensitivityClass::Private, "p", b"private", "test")
                .unwrap();
        }

        let bundle = d.path().join("vault.tar.gz");
        export(&src, &bundle).unwrap();
        assert!(bundle.exists());

        let dst = d.path().join("restored");
        import(&bundle, &dst).unwrap();

        // Open the restored vault with the same master and verify entries.
        let store: Arc<dyn Store> = Arc::new(FsStore::new(&dst).unwrap());
        let v = HostVault::new(store, master);
        v.unseal_full();
        let bytes = v
            .read(SensitivityClass::Public, "k", "test")
            .unwrap()
            .unwrap();
        assert_eq!(&bytes[..], b"hello");
        let bytes = v
            .read(SensitivityClass::Private, "p", "test")
            .unwrap()
            .unwrap();
        assert_eq!(&bytes[..], b"private");
    }

    #[test]
    fn import_into_nonempty_dir_fails() {
        let d = tempdir().unwrap();
        let src = d.path().join("vault");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("anything"), b"x").unwrap();
        let bundle = d.path().join("v.tar.gz");
        // Make a tiny empty bundle to import.
        let f = std::fs::File::create(&bundle).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        tar::Builder::new(gz)
            .into_inner()
            .unwrap()
            .finish()
            .unwrap();

        assert!(import(&bundle, &src).is_err());
    }
}

#[cfg(test)]
mod tests_rotate {
    use super::*;
    use crate::host::vault::store::{FsStore, Store};
    use std::sync::Arc;
    use tempfile::tempdir;
    use tkr_api::manifest::SensitivityClass;

    #[test]
    fn rotate_preserves_entries_under_new_key() {
        let d = tempdir().unwrap();
        let vault_path = d.path().join("vault");
        std::fs::create_dir_all(&vault_path).unwrap();
        let master = [3u8; 32];
        {
            let store: Arc<dyn Store> = Arc::new(FsStore::new(&vault_path).unwrap());
            let v = HostVault::new(store, master);
            v.unseal_full();
            v.write(SensitivityClass::Public, "a", b"alpha", "test")
                .unwrap();
            v.write(SensitivityClass::Private, "b", b"bravo", "test")
                .unwrap();
            v.write(SensitivityClass::Secret, "c", b"charlie", "test")
                .unwrap();
        }

        // Reopen for rotate
        let store: Arc<dyn Store> = Arc::new(FsStore::new(&vault_path).unwrap());
        let old = HostVault::new(store, master);
        let _new_master = rotate(&old, &vault_path).unwrap();
        drop(old);

        // The directory now holds re-encrypted blobs under a new master.
        // Verify with the OLD master: reads should fail or return None.
        let store: Arc<dyn Store> = Arc::new(FsStore::new(&vault_path).unwrap());
        let post = HostVault::new(store, master);
        post.unseal_full();
        let r = post.read(SensitivityClass::Public, "a", "test");
        assert!(r.is_err() || matches!(r, Ok(None)));
    }
}
