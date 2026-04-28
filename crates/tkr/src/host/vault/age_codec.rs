use age::secrecy::SecretString;
use anyhow::{anyhow, Context, Result};
use std::io::{Read, Write};

fn key_to_passphrase(key: &[u8; 32]) -> SecretString {
    SecretString::new(hex::encode(key))
}

pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>> {
    let enc = age::Encryptor::with_user_passphrase(key_to_passphrase(key));
    let mut out = Vec::new();
    let mut writer = enc.wrap_output(&mut out).context("wrap output")?;
    writer.write_all(plaintext)?;
    writer.finish()?;
    Ok(out)
}

pub fn decrypt(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let dec = match age::Decryptor::new(ciphertext).context("parse age header")? {
        age::Decryptor::Passphrase(d) => d,
        age::Decryptor::Recipients(_) => {
            return Err(anyhow!("unexpected recipients ciphertext"))
        }
    };
    let mut reader = dec
        .decrypt(&key_to_passphrase(key), None)
        .context("decrypt")?;
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = [7u8; 32];
        let ct = encrypt(&key, b"hello").unwrap();
        assert_eq!(decrypt(&key, &ct).unwrap(), b"hello".to_vec());
    }

    #[test]
    fn wrong_key_fails() {
        let ct = encrypt(&[7u8; 32], b"x").unwrap();
        assert!(decrypt(&[8u8; 32], &ct).is_err());
    }

    #[test]
    fn empty_plaintext_round_trips() {
        let ct = encrypt(&[1u8; 32], b"").unwrap();
        assert_eq!(decrypt(&[1u8; 32], &ct).unwrap(), Vec::<u8>::new());
    }
}
