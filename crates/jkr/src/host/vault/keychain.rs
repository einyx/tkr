//! Master key persistence for the jkr vault.
//!
//! As of v0.2.6, the master key lives in ~/.jkr/vault/.<service>.key with
//! mode 0600. Previously it was kept in the macOS login keychain via the
//! `keyring` crate, but the default ACL there scopes access by cdhash, so
//! every new binary build couldn't decrypt data written by the previous one.
//! On first run after upgrade, the legacy keychain item (if any) is migrated
//! to the file. The keychain is no longer the source of truth.
//!
//! Threat model: someone with read access to your user's $HOME can take the
//! key. The vault contents (jkr's own analytics noise signatures) are not
//! material to most threat models. If you need stronger protection, replace
//! this module's persistence with a Secure-Enclave-encrypted blob (macOS) or
//! a TPM-sealed key (Linux); the public API stays the same.

use anyhow::{Context, Result};
use std::path::PathBuf;

#[allow(dead_code)]
pub const SERVICE_DEFAULT: &str = "jkr-vault";

fn key_path(service: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".jkr")
        .join("vault")
        .join(format!(".{service}.key"))
}

pub fn set_master_key(service: &str, _user: &str, key: &[u8]) -> Result<()> {
    let path = key_path(service);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, key).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&path)?.permissions();
        perm.set_mode(0o600);
        std::fs::set_permissions(&path, perm)?;
    }
    Ok(())
}

pub fn get_master_key(service: &str, user: &str) -> Result<Vec<u8>> {
    let path = key_path(service);
    if path.exists() {
        return std::fs::read(&path).with_context(|| format!("read {}", path.display()));
    }
    // Legacy migration path: if the file doesn't exist but the keychain has
    // an item, copy it to disk and use it. This makes upgrade seamless for
    // users on the old keychain backend.
    if let Ok(legacy) = legacy_keyring_get(service, user) {
        let _ = set_master_key(service, user, &legacy);
        return Ok(legacy);
    }
    anyhow::bail!("master key not found for service '{service}'")
}

pub fn delete_master_key(service: &str, _user: &str) -> Result<()> {
    let path = key_path(service);
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("delete {}", path.display()))?;
    }
    Ok(())
}

pub fn init_master_key_if_missing(service: &str, user: &str) -> Result<Vec<u8>> {
    if let Ok(k) = get_master_key(service, user) {
        return Ok(k);
    }
    let mut buf = vec![0u8; 32];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut buf);
    set_master_key(service, user, &buf)?;
    Ok(buf)
}

fn legacy_keyring_get(service: &str, user: &str) -> Result<Vec<u8>> {
    keyring::Entry::new(service, user)?
        .get_secret()
        .context("legacy keyring read")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_based_round_trip() {
        let svc = "jkr-test-file-key";
        let user = "master";
        // Clean up from any previous run.
        let _ = delete_master_key(svc, user);
        set_master_key(svc, user, b"hello").unwrap();
        assert_eq!(get_master_key(svc, user).unwrap(), b"hello");
        delete_master_key(svc, user).unwrap();
        // Confirm deletion.
        assert!(get_master_key(svc, user).is_err());
    }

    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let svc = "jkr-test-keychain";
        let user = "master";
        let _ = delete_master_key(svc, user);
        set_master_key(svc, user, b"hello").unwrap();
        assert_eq!(get_master_key(svc, user).unwrap(), b"hello");
        delete_master_key(svc, user).unwrap();
    }
}
