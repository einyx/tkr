//! secp256k1 identity. Same key shape as an Ethereum wallet — the EIP-55
//! address derived from the public key is the on-mesh address.

use crate::{Address, Error, Result};
use k256::ecdsa::{RecoveryId, Signature, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use sha3::{Digest, Keccak256};

/// A peer identity: secp256k1 keypair + cached EIP-55 address.
#[derive(Debug, Clone)]
pub struct Identity {
    signing: SigningKey,
    address: Address,
}

impl Identity {
    /// Generate a fresh identity from the OS RNG.
    pub fn generate() -> Self {
        let signing = SigningKey::random(&mut OsRng);
        let address = address_from_verifying(signing.verifying_key());
        Self { signing, address }
    }

    /// Load an identity from a 32-byte private key.
    pub fn from_secret_bytes(bytes: &[u8; 32]) -> Result<Self> {
        let signing = SigningKey::from_slice(bytes)
            .map_err(|e| Error::Crypto(format!("invalid secret key: {e}")))?;
        let address = address_from_verifying(signing.verifying_key());
        Ok(Self { signing, address })
    }

    pub fn address(&self) -> Address {
        self.address
    }

    pub fn verifying_key(&self) -> &VerifyingKey {
        self.signing.verifying_key()
    }

    /// 32-byte private key. Treat as a secret.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes().into()
    }

    /// Sign a 32-byte digest. Returns a 65-byte Ethereum-style signature
    /// (r || s || v) where v ∈ {27, 28} — compatible with `ecrecover`.
    pub fn sign_digest(&self, digest: &[u8; 32]) -> [u8; 65] {
        let (sig, recid) = self
            .signing
            .sign_prehash_recoverable(digest)
            .expect("sign_prehash on a 32-byte digest cannot fail");
        encode_eth_signature(&sig, recid)
    }
}

/// Derive an EIP-55 address from a verifying key:
/// keccak256(uncompressed_pubkey[1..])[12..] → 20 bytes.
fn address_from_verifying(vk: &VerifyingKey) -> Address {
    let encoded = vk.to_encoded_point(false); // uncompressed: 0x04 || X || Y
    let pub_bytes = &encoded.as_bytes()[1..]; // strip 0x04 prefix
    let hash = Keccak256::digest(pub_bytes);
    let mut out = [0u8; 20];
    out.copy_from_slice(&hash[12..]);
    Address::from_bytes(out)
}

fn encode_eth_signature(sig: &Signature, recid: RecoveryId) -> [u8; 65] {
    let mut out = [0u8; 65];
    let bytes = sig.to_bytes();
    out[..64].copy_from_slice(&bytes);
    // Ethereum v = 27 + recid (legacy; pre-EIP-155 / non-tx signing).
    out[64] = 27 + recid.to_byte();
    out
}

/// Recover the signer's address from a 65-byte Ethereum-style signature
/// over a 32-byte digest. Used by verifiers (e.g. invite check) that don't
/// have the signer's pubkey ahead of time.
pub fn recover_address(digest: &[u8; 32], signature: &[u8; 65]) -> Result<Address> {
    let v = signature[64];
    let recid_byte = v
        .checked_sub(27)
        .ok_or_else(|| Error::BadSignature)?;
    let recid = RecoveryId::from_byte(recid_byte).ok_or(Error::BadSignature)?;
    let sig = Signature::from_slice(&signature[..64]).map_err(|_| Error::BadSignature)?;
    let vk = VerifyingKey::recover_from_prehash(digest, &sig, recid)
        .map_err(|_| Error::BadSignature)?;
    Ok(address_from_verifying(&vk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_yields_well_formed_address() {
        let id = Identity::generate();
        let addr = id.address().to_checksum();
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 42);
    }

    #[test]
    fn round_trip_secret_bytes() {
        let id = Identity::generate();
        let bytes = id.secret_bytes();
        let again = Identity::from_secret_bytes(&bytes).unwrap();
        assert_eq!(id.address(), again.address());
    }

    #[test]
    fn sign_recover_matches_signer() {
        let id = Identity::generate();
        let digest = [0x42u8; 32];
        let sig = id.sign_digest(&digest);
        let recovered = recover_address(&digest, &sig).unwrap();
        assert_eq!(recovered, id.address());
    }

    #[test]
    fn recover_fails_on_wrong_digest() {
        let id = Identity::generate();
        let digest = [0x42u8; 32];
        let sig = id.sign_digest(&digest);
        let other_digest = [0x43u8; 32];
        let recovered = recover_address(&other_digest, &sig).unwrap();
        assert_ne!(recovered, id.address());
    }

    /// Reference vector: a known private key must yield a known address.
    /// Source: standard test vector — secret 0x01 → address 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf.
    #[test]
    fn known_key_yields_known_address() {
        let mut secret = [0u8; 32];
        secret[31] = 1;
        let id = Identity::from_secret_bytes(&secret).unwrap();
        assert_eq!(
            id.address().to_checksum(),
            "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf"
        );
    }
}
