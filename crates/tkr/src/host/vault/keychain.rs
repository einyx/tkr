use anyhow::{Context, Result};
use keyring::Entry;

#[allow(dead_code)]
pub const SERVICE_DEFAULT: &str = "tkr-vault";

pub fn set_master_key(service: &str, user: &str, key: &[u8]) -> Result<()> {
    Entry::new(service, user)?.set_secret(key).context("write keychain")
}

pub fn get_master_key(service: &str, user: &str) -> Result<Vec<u8>> {
    Ok(Entry::new(service, user)?.get_secret().context("read keychain")?)
}

pub fn delete_master_key(service: &str, user: &str) -> Result<()> {
    Ok(Entry::new(service, user)?
        .delete_credential()
        .context("delete keychain")?)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let svc = "tkr-test-keychain";
        let user = "master";
        let _ = delete_master_key(svc, user);
        set_master_key(svc, user, b"hello").unwrap();
        assert_eq!(get_master_key(svc, user).unwrap(), b"hello");
        delete_master_key(svc, user).unwrap();
    }
}
