//! Vault encryption codec.
//!
//! The old format used `age` with `Encryptor::with_user_passphrase`, which
//! routes through scrypt with a "1 second target work factor". That made every
//! single vault read/write run scrypt — and since the analytics path calls
//! `HostVault::read` per signature recorded, hot commands like `tkr ls`
//! spent 10+ seconds doing scrypt KDFs and appeared to hang.
//!
//! The subkey passed in here is already a high-entropy 32-byte key derived
//! from the master via HKDF. There is no need for a password-based KDF on
//! every operation. We use XChaCha20-Poly1305 directly.
//!
//! On read we sniff the magic so existing vaults written by the old age-based
//! codec keep working; the next write rewrites them in the fast format.
//!
//! Module name kept (`age_codec`) to avoid touching every call-site.
use anyhow::{anyhow, Context, Result};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use std::io::Read;
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"TKR1";
const NONCE_LEN: usize = 24;

pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|e| anyhow!("aead encrypt: {e}"))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt `ciphertext` and return plaintext wrapped in `Zeroizing` so the
/// buffer is wiped on drop.
pub fn decrypt(key: &[u8; 32], ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if ciphertext.len() >= MAGIC.len() + NONCE_LEN && &ciphertext[..MAGIC.len()] == MAGIC {
        let cipher = XChaCha20Poly1305::new(key.into());
        let nonce = &ciphertext[MAGIC.len()..MAGIC.len() + NONCE_LEN];
        let body = &ciphertext[MAGIC.len() + NONCE_LEN..];
        let pt = cipher
            .decrypt(XNonce::from_slice(nonce), body)
            .map_err(|e| anyhow!("aead decrypt: {e}"))?;
        return Ok(Zeroizing::new(pt));
    }
    decrypt_legacy_age(key, ciphertext)
}

/// Backwards-compat path for vaults written by the old age-based codec.
/// Slow (runs scrypt) but only hit once per blob; the next write replaces it
/// with the fast format.
fn decrypt_legacy_age(key: &[u8; 32], ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    use age::secrecy::SecretString;
    let dec = match age::Decryptor::new(ciphertext).context("parse age header")? {
        age::Decryptor::Passphrase(d) => d,
        age::Decryptor::Recipients(_) => {
            return Err(anyhow!("unexpected recipients ciphertext"))
        }
    };
    let passphrase = SecretString::new(hex::encode(key));
    let mut reader = dec.decrypt(&passphrase, None).context("decrypt")?;
    let mut out = Zeroizing::new(Vec::new());
    reader.read_to_end(&mut *out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = [7u8; 32];
        let ct = encrypt(&key, b"hello").unwrap();
        assert_eq!(*decrypt(&key, &ct).unwrap(), b"hello".to_vec());
    }

    #[test]
    fn wrong_key_fails() {
        let ct = encrypt(&[7u8; 32], b"x").unwrap();
        assert!(decrypt(&[8u8; 32], &ct).is_err());
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let ct = encrypt(&[1u8; 32], b"").unwrap();
        assert_eq!(*decrypt(&[1u8; 32], &ct).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn nonce_is_unique_per_call() {
        let key = [9u8; 32];
        let a = encrypt(&key, b"same plaintext").unwrap();
        let b = encrypt(&key, b"same plaintext").unwrap();
        assert_ne!(a, b, "nonce must randomize ciphertext");
    }

    #[test]
    fn legacy_age_blob_still_decrypts() {
        // Encrypt with the old age path directly so we don't depend on the
        // public API exposing it.
        use age::secrecy::SecretString;
        use std::io::Write;
        let key = [3u8; 32];
        let passphrase = SecretString::new(hex::encode(key));
        let enc = age::Encryptor::with_user_passphrase(passphrase);
        let mut legacy = Vec::new();
        let mut w = enc.wrap_output(&mut legacy).unwrap();
        w.write_all(b"legacy payload").unwrap();
        w.finish().unwrap();
        assert_eq!(*decrypt(&key, &legacy).unwrap(), b"legacy payload".to_vec());
    }
}
