use anyhow::Result;
use tkr_api::vault::{SealState, Vault as VaultTrait};
use crate::host::vault::HostVault;

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
    Keychain,
    Passphrase(&'a str),
}

pub fn init(vault_root: &Path, mode: InitMode) -> Result<i32> {
    use anyhow::Context;
    use rand::RngCore;
    use crate::host::vault::keychain;

    std::fs::create_dir_all(vault_root).context("create vault dir")?;
    let mut master = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut master);

    match mode {
        InitMode::Keychain => {
            let user = vault_root.to_string_lossy();
            // If keychain already has a master, leave it alone (idempotent).
            if keychain::get_master_key("tkr-vault", &user).is_ok() {
                println!(
                    "vault already initialized at {} (keychain entry exists)",
                    vault_root.display()
                );
                return Ok(0);
            }
            keychain::set_master_key("tkr-vault", &user, &master)?;
            println!(
                "vault initialized at {} (master key stored in OS keychain)",
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
            let enc =
                age::Encryptor::with_user_passphrase(SecretString::new(p.to_string()));
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
    use tempfile::tempdir;
    use crate::host::vault::keychain;

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

    // Keychain test is #[ignore]'d like the existing keychain_round_trip — touches OS keyring.
    #[test]
    #[ignore]
    fn init_keychain_creates_entry() {
        let d = tempdir().unwrap();
        let user = d.path().to_string_lossy().to_string();
        let _ = keychain::delete_master_key("tkr-vault", &user);
        init(d.path(), InitMode::Keychain).unwrap();
        let key = keychain::get_master_key("tkr-vault", &user).unwrap();
        assert_eq!(key.len(), 32);
        let _ = keychain::delete_master_key("tkr-vault", &user);
    }
}

#[cfg(test)]
mod tests_status {
    use super::*;
    use std::sync::Arc;
    use crate::host::vault::store::{MemStore, Store};

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
    use tkr_api::vault::SealState;
    use std::sync::Arc;
    use crate::host::vault::store::{MemStore, Store};

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
