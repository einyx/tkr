//! Joiner-side proof of address control for `POST /api/v1/mesh/join`.
//!
//! An invite is an unauthenticated bearer token: anyone holding it can
//! present it to the broker. To stop a leaked invite from being used to
//! enroll arbitrary `address` values (which would let an attacker either
//! impersonate planned future members or pad the membership with their
//! own keypairs), the joiner must additionally prove they hold the
//! private key for `address` by signing a fresh `JoinAttestation`.
//!
//! The attestation binds:
//! - the claimed `mesh_id` (so a sig for one mesh can't be replayed at
//!   another),
//! - the joiner's `address` (recovered from the signature must match),
//! - `keccak256(invite_token)` (so a sig for one invite can't be
//!   replayed against a different one — even if the same joiner key
//!   signs many),
//! - a wall-clock `timestamp_ms` (so a captured attestation can't be
//!   replayed across long timescales — broker enforces a freshness
//!   window).

use crate::{identity::recover_address, Address, Error, Identity, Result};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

/// Max accepted skew between joiner-supplied `timestamp_ms` and broker
/// wall clock, in either direction. Wider than the WS hello window
/// (60s) because HTTP enrollment can sit behind retries / proxies, but
/// still tight enough that a sig leaked an hour later is useless.
pub const JOIN_ATTESTATION_MAX_SKEW_MS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct JoinAttestation {
    pub mesh_id: String,
    pub address: Address,
    /// Lowercase hex of `keccak256(invite_token bytes)`. Binds this
    /// attestation to the specific invite the joiner is redeeming.
    pub invite_token_hash: String,
    pub timestamp_ms: u64,
    /// 65-byte Ethereum signature, hex (with or without `0x` prefix).
    pub signature: String,
}

impl JoinAttestation {
    /// Build and sign an attestation for `(mesh_id, identity.address(),
    /// invite_token, timestamp_ms)`.
    pub fn issue(
        identity: &Identity,
        mesh_id: impl Into<String>,
        invite_token: &str,
        timestamp_ms: u64,
    ) -> Self {
        let mesh_id = mesh_id.into();
        let address = identity.address();
        let token_hash = keccak256_hex(invite_token.as_bytes());
        let digest = attestation_digest(&mesh_id, &address, &token_hash, timestamp_ms);
        let sig = identity.sign_digest(&digest);
        Self {
            mesh_id,
            address,
            invite_token_hash: token_hash,
            timestamp_ms,
            signature: format!("0x{}", hex::encode(sig)),
        }
    }

    /// Verify signature recovers to the embedded `address` and that
    /// `invite_token_hash` matches `keccak256(expected_invite_token)`.
    /// Does **not** check freshness — call [`Self::verify_with_now`] in
    /// any networked context.
    pub fn verify(&self, expected_invite_token: &str) -> Result<()> {
        let expected_hash = keccak256_hex(expected_invite_token.as_bytes());
        if self.invite_token_hash != expected_hash {
            return Err(Error::Encoding(
                "join attestation invite_token_hash mismatch".to_string(),
            ));
        }
        let digest = attestation_digest(
            &self.mesh_id,
            &self.address,
            &self.invite_token_hash,
            self.timestamp_ms,
        );
        let sig_bytes = parse_signature(&self.signature)?;
        let recovered = recover_address(&digest, &sig_bytes)?;
        if recovered != self.address {
            return Err(Error::BadSignature);
        }
        Ok(())
    }

    /// Verify signature + invite-token binding, **and** that
    /// `timestamp_ms` is within `max_skew_ms` of `now_ms`.
    pub fn verify_with_now(
        &self,
        expected_invite_token: &str,
        now_ms: u64,
        max_skew_ms: u64,
    ) -> Result<()> {
        self.verify(expected_invite_token)?;
        let skew = self.timestamp_ms.abs_diff(now_ms);
        if skew > max_skew_ms {
            return Err(Error::Encoding(format!(
                "join attestation timestamp out of window: skew={skew}ms, max={max_skew_ms}ms"
            )));
        }
        Ok(())
    }
}

fn attestation_digest(mesh_id: &str, addr: &Address, token_hash_hex: &str, ts_ms: u64) -> [u8; 32] {
    // Domain-tagged keccak so an attestation signature can never be
    // confused with a Hello, an EIP-712 invite, or a payment receipt.
    let mut h = Keccak256::new();
    h.update(b"tkr-mesh/v1/join-attestation\n");
    h.update(mesh_id.as_bytes());
    h.update(b"\n");
    h.update(addr.as_bytes());
    h.update(b"\n");
    h.update(token_hash_hex.as_bytes());
    h.update(b"\n");
    h.update(ts_ms.to_be_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

fn keccak256_hex(bytes: &[u8]) -> String {
    let digest = Keccak256::digest(bytes);
    hex::encode(digest)
}

fn parse_signature(s: &str) -> Result<[u8; 65]> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped)
        .map_err(|e| Error::Encoding(format!("signature hex: {e}")))?;
    if bytes.len() != 65 {
        return Err(Error::Encoding(format!(
            "signature: expected 65 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 65];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_now() -> u64 {
        1_700_000_000_000
    }

    #[test]
    fn issue_then_verify_succeeds() {
        let id = Identity::generate();
        let att = JoinAttestation::issue(&id, "mesh_x", "tok_abc", fresh_now());
        att.verify("tok_abc").expect("verify");
        att.verify_with_now("tok_abc", fresh_now(), JOIN_ATTESTATION_MAX_SKEW_MS)
            .expect("verify_with_now");
    }

    #[test]
    fn wrong_invite_token_rejected() {
        let id = Identity::generate();
        let att = JoinAttestation::issue(&id, "mesh_x", "tok_abc", fresh_now());
        let err = att.verify("tok_DIFFERENT").unwrap_err();
        assert!(matches!(err, Error::Encoding(_)), "got: {err:?}");
    }

    #[test]
    fn forged_address_rejected() {
        let signer = Identity::generate();
        let other = Identity::generate();
        assert_ne!(signer.address(), other.address());

        let mut att = JoinAttestation::issue(&signer, "mesh_x", "tok", fresh_now());
        // Swap in a different address — recovery will return signer.address(),
        // which will no longer equal the embedded `att.address`.
        att.address = other.address();
        assert!(matches!(att.verify("tok").unwrap_err(), Error::BadSignature));
    }

    #[test]
    fn tampered_mesh_id_rejected() {
        let id = Identity::generate();
        let mut att = JoinAttestation::issue(&id, "mesh_x", "tok", fresh_now());
        att.mesh_id = "mesh_evil".to_string();
        assert!(matches!(att.verify("tok").unwrap_err(), Error::BadSignature));
    }

    #[test]
    fn stale_timestamp_rejected() {
        let id = Identity::generate();
        let att = JoinAttestation::issue(&id, "mesh_x", "tok", fresh_now());
        let too_late = fresh_now() + JOIN_ATTESTATION_MAX_SKEW_MS + 1;
        let err = att
            .verify_with_now("tok", too_late, JOIN_ATTESTATION_MAX_SKEW_MS)
            .unwrap_err();
        assert!(matches!(err, Error::Encoding(_)), "got: {err:?}");
    }

    #[test]
    fn future_timestamp_outside_window_rejected() {
        let id = Identity::generate();
        let now = fresh_now();
        let future_ts = now + JOIN_ATTESTATION_MAX_SKEW_MS + 1;
        let att = JoinAttestation::issue(&id, "mesh_x", "tok", future_ts);
        assert!(att
            .verify_with_now("tok", now, JOIN_ATTESTATION_MAX_SKEW_MS)
            .is_err());
    }

    #[test]
    fn cross_mesh_replay_rejected() {
        // Attacker captures attestation for mesh_a, swaps mesh_id to mesh_b.
        let id = Identity::generate();
        let mut att = JoinAttestation::issue(&id, "mesh_a", "tok", fresh_now());
        att.mesh_id = "mesh_b".to_string();
        assert!(att.verify("tok").is_err());
    }
}
