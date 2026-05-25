//! ECIES envelope for direct messages.
//!
//! Construction: ECDH(secp256k1) → HKDF-SHA256 → AES-256-GCM.
//!
//! Sender generates a fresh ephemeral keypair per message; the ephemeral
//! public key travels with the ciphertext so the receiver can derive the
//! shared secret. Because the AES key is unique per message, GCM nonces
//! can be a fixed value (we use 12 zero bytes) without risking nonce reuse.
//! AAD is bound to the recipient address so a ciphertext addressed to one
//! peer cannot be replayed against another.

use crate::{Address, Error, Identity, Result};
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hkdf::Hkdf;
use k256::ecdh::diffie_hellman;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::{PublicKey, SecretKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

const HKDF_INFO: &[u8] = b"jkr-mesh/v1/dm";
/// Per-message AES key + 0-nonce: nonce-reuse impossible.
const ZERO_NONCE: [u8; 12] = [0u8; 12];

/// Sealed direct-message envelope. Wire-friendly: all binary fields are
/// hex strings so it embeds in JSON frames cleanly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope {
    /// 33-byte compressed secp256k1 public key, hex.
    pub eph_pub: String,
    /// AES-GCM ciphertext (tag appended), hex.
    pub ciphertext: String,
}

impl Envelope {
    /// Encrypt `plaintext` for `recipient`. Generates a fresh ephemeral
    /// keypair internally — no caller-managed nonce or KDF state.
    pub fn seal(plaintext: &[u8], recipient_pub: &PublicKey, recipient: Address) -> Result<Self> {
        let eph_secret = SecretKey::random(&mut OsRng);
        let eph_pub = eph_secret.public_key();

        let shared = diffie_hellman(eph_secret.to_nonzero_scalar(), recipient_pub.as_affine());
        let key = derive_key(shared.raw_secret_bytes().as_slice())?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));

        let aad = recipient_aad(&recipient);
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&ZERO_NONCE),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|e| Error::Crypto(format!("aes-gcm encrypt: {e}")))?;

        Ok(Envelope {
            eph_pub: hex::encode(eph_pub.to_encoded_point(true).as_bytes()),
            ciphertext: hex::encode(ct),
        })
    }

    /// Decrypt with `recipient_identity`'s key. AAD binds the envelope to
    /// the receiver's address — a malformed/redirected envelope fails the
    /// AEAD tag check.
    pub fn open(&self, recipient_identity: &Identity) -> Result<Vec<u8>> {
        let eph_pub_bytes =
            hex::decode(&self.eph_pub).map_err(|e| Error::Encoding(format!("eph_pub hex: {e}")))?;
        let eph_pub = PublicKey::from_sec1_bytes(&eph_pub_bytes)
            .map_err(|e| Error::Crypto(format!("eph_pub parse: {e}")))?;

        let recipient_secret = SecretKey::from_slice(&recipient_identity.secret_bytes())
            .map_err(|e| Error::Crypto(format!("recipient secret: {e}")))?;
        let shared = diffie_hellman(recipient_secret.to_nonzero_scalar(), eph_pub.as_affine());
        let key = derive_key(shared.raw_secret_bytes().as_slice())?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));

        let ct = hex::decode(&self.ciphertext)
            .map_err(|e| Error::Encoding(format!("ciphertext hex: {e}")))?;
        let aad = recipient_aad(&recipient_identity.address());
        cipher
            .decrypt(
                Nonce::from_slice(&ZERO_NONCE),
                Payload {
                    msg: &ct,
                    aad: &aad,
                },
            )
            .map_err(|_| Error::BadSignature) // AEAD failure — opaque to attackers
    }
}

fn derive_key(shared_secret: &[u8]) -> Result<[u8; 32]> {
    let hk = Hkdf::<Sha256>::new(None, shared_secret);
    let mut okm = [0u8; 32];
    hk.expand(HKDF_INFO, &mut okm)
        .map_err(|e| Error::Crypto(format!("hkdf: {e}")))?;
    Ok(okm)
}

fn recipient_aad(addr: &Address) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 20);
    out.extend_from_slice(b"jkr-mesh");
    out.extend_from_slice(addr.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::SecretKey;

    fn pubkey_of(id: &Identity) -> PublicKey {
        let sk = SecretKey::from_slice(&id.secret_bytes()).unwrap();
        sk.public_key()
    }

    #[test]
    fn seal_open_round_trip() {
        let bob = Identity::generate();
        let plaintext = b"hello bob, this is alice";
        let env = Envelope::seal(plaintext, &pubkey_of(&bob), bob.address()).unwrap();
        let decoded = env.open(&bob).unwrap();
        assert_eq!(decoded, plaintext);
    }

    #[test]
    fn wrong_recipient_fails() {
        let bob = Identity::generate();
        let eve = Identity::generate();
        let env = Envelope::seal(b"for bob only", &pubkey_of(&bob), bob.address()).unwrap();
        let err = env.open(&eve).unwrap_err();
        assert!(matches!(err, Error::BadSignature));
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let bob = Identity::generate();
        let mut env = Envelope::seal(b"hello", &pubkey_of(&bob), bob.address()).unwrap();
        // Flip a hex char inside the ciphertext (skip past tag prefix to
        // ensure we hit ciphertext bytes deterministically).
        let mut bytes: Vec<u8> = env.ciphertext.bytes().collect();
        bytes[0] = if bytes[0] == b'a' { b'b' } else { b'a' };
        env.ciphertext = String::from_utf8(bytes).unwrap();
        assert!(env.open(&bob).is_err());
    }

    #[test]
    fn redirected_envelope_fails() {
        // An attacker tries to take an envelope sealed for Bob and present
        // it as if it were addressed to Charlie. Bob can't decrypt
        // (different key), and the AAD binds to the recipient address so
        // even if Bob's key were leaked Charlie's daemon would reject it.
        let bob = Identity::generate();
        let charlie = Identity::generate();
        // Seal for charlie's pubkey but with bob's address as the AAD.
        let env =
            Envelope::seal(b"intended for charlie", &pubkey_of(&charlie), bob.address()).unwrap();
        // Charlie's daemon (correctly) tries to open with charlie's address as AAD.
        assert!(env.open(&charlie).is_err());
    }

    #[test]
    fn each_seal_uses_fresh_ephemeral() {
        let bob = Identity::generate();
        let env1 = Envelope::seal(b"same msg", &pubkey_of(&bob), bob.address()).unwrap();
        let env2 = Envelope::seal(b"same msg", &pubkey_of(&bob), bob.address()).unwrap();
        assert_ne!(env1.eph_pub, env2.eph_pub);
        assert_ne!(env1.ciphertext, env2.ciphertext);
    }

    #[test]
    fn json_round_trip() {
        let bob = Identity::generate();
        let env = Envelope::seal(b"persisted", &pubkey_of(&bob), bob.address()).unwrap();
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.open(&bob).unwrap(), b"persisted");
    }
}
