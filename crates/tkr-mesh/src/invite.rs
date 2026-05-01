//! Mesh invites. EIP-712 typed-data signed by the mesh owner; renders
//! human-readably in any wallet that supports `eth_signTypedData_v4`.

use crate::{identity::recover_address, Address, Error, Identity, Result};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
}

impl Role {
    fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }
}

/// Invite payload. After EIP-712 signing it is base64url-wrapped into an
/// `https://<host>/join/<token>` URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub v: u32,
    pub mesh_id: String,
    pub mesh_slug: String,
    pub broker_url: String,
    pub expires_at: u64,
    pub role: Role,
    pub owner: Address,
    /// 65-byte Ethereum signature, hex-encoded with `0x` prefix.
    pub signature: String,
}

impl Invite {
    /// Create and sign an invite. `mesh_id` is opaque (caller picks ULID,
    /// UUID, etc). `expires_at` is unix seconds.
    pub fn issue(
        owner_identity: &Identity,
        mesh_id: impl Into<String>,
        mesh_slug: impl Into<String>,
        broker_url: impl Into<String>,
        expires_at: u64,
        role: Role,
    ) -> Self {
        let mut invite = Invite {
            v: 1,
            mesh_id: mesh_id.into(),
            mesh_slug: mesh_slug.into(),
            broker_url: broker_url.into(),
            expires_at,
            role,
            owner: owner_identity.address(),
            signature: String::new(),
        };
        let digest = invite.eip712_digest();
        let sig = owner_identity.sign_digest(&digest);
        invite.signature = format!("0x{}", hex::encode(sig));
        invite
    }

    /// Verify signature, owner address consistency, and expiry. `now` is
    /// unix seconds (caller-supplied to keep the function deterministic for
    /// tests).
    pub fn verify(&self, now: u64) -> Result<()> {
        if self.v != 1 {
            return Err(Error::InvalidInvite(format!(
                "unsupported version {}",
                self.v
            )));
        }
        if self.expires_at <= now {
            return Err(Error::Expired {
                expires_at: self.expires_at,
                now,
            });
        }
        let sig_bytes = parse_signature(&self.signature)?;
        let digest = self.eip712_digest();
        let recovered = recover_address(&digest, &sig_bytes)?;
        if recovered != self.owner {
            return Err(Error::BadSignature);
        }
        Ok(())
    }

    /// EIP-712 digest: keccak256("\x19\x01" || domainSeparator || hashStruct(Invite)).
    fn eip712_digest(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(2 + 32 + 32);
        buf.extend_from_slice(b"\x19\x01");
        buf.extend_from_slice(&domain_separator());
        buf.extend_from_slice(&self.struct_hash());
        let mut out = [0u8; 32];
        out.copy_from_slice(&Keccak256::digest(&buf));
        out
    }

    fn struct_hash(&self) -> [u8; 32] {
        // typeHash = keccak256("Invite(uint32 v,string meshId,string meshSlug,string brokerUrl,uint64 expiresAt,string role,address owner)")
        let type_hash = Keccak256::digest(
            b"Invite(uint32 v,string meshId,string meshSlug,string brokerUrl,uint64 expiresAt,string role,address owner)"
        );
        let mut buf = Vec::with_capacity(7 * 32);
        buf.extend_from_slice(&type_hash);
        buf.extend_from_slice(&u256_be(u64::from(self.v)));
        buf.extend_from_slice(&Keccak256::digest(self.mesh_id.as_bytes()));
        buf.extend_from_slice(&Keccak256::digest(self.mesh_slug.as_bytes()));
        buf.extend_from_slice(&Keccak256::digest(self.broker_url.as_bytes()));
        buf.extend_from_slice(&u256_be(self.expires_at));
        buf.extend_from_slice(&Keccak256::digest(self.role.as_str().as_bytes()));
        buf.extend_from_slice(&address_padded(&self.owner));
        let mut out = [0u8; 32];
        out.copy_from_slice(&Keccak256::digest(&buf));
        out
    }
}

/// EIP-712 domain. Pinned constants — change → invalidates all old invites.
fn domain_separator() -> [u8; 32] {
    // typeHash = keccak256("EIP712Domain(string name,string version)")
    let type_hash = Keccak256::digest(b"EIP712Domain(string name,string version)");
    let mut buf = Vec::with_capacity(3 * 32);
    buf.extend_from_slice(&type_hash);
    buf.extend_from_slice(&Keccak256::digest(b"tkr-mesh"));
    buf.extend_from_slice(&Keccak256::digest(b"1"));
    let mut out = [0u8; 32];
    out.copy_from_slice(&Keccak256::digest(&buf));
    out
}

fn u256_be(n: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[24..].copy_from_slice(&n.to_be_bytes());
    out
}

fn address_padded(addr: &Address) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(addr.as_bytes());
    out
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

    #[test]
    fn issue_then_verify_succeeds() {
        let owner = Identity::generate();
        let invite = Invite::issue(
            &owner,
            "mesh_01HX",
            "acme",
            "wss://broker.example/ws",
            2_000_000_000,
            Role::Member,
        );
        invite.verify(1_700_000_000).expect("verify");
    }

    #[test]
    fn expired_invite_rejected() {
        let owner = Identity::generate();
        let invite = Invite::issue(
            &owner,
            "mesh_01HX",
            "acme",
            "wss://broker.example/ws",
            1_000,
            Role::Member,
        );
        let err = invite.verify(2_000).unwrap_err();
        assert!(matches!(err, Error::Expired { .. }));
    }

    #[test]
    fn tampered_field_breaks_signature() {
        let owner = Identity::generate();
        let mut invite = Invite::issue(
            &owner,
            "mesh_01HX",
            "acme",
            "wss://broker.example/ws",
            2_000_000_000,
            Role::Member,
        );
        invite.mesh_slug = "evil".to_string();
        let err = invite.verify(1_700_000_000).unwrap_err();
        assert!(matches!(err, Error::BadSignature));
    }

    #[test]
    fn fake_owner_rejected() {
        // Sign with one key but claim the invite is owned by another.
        let real_owner = Identity::generate();
        let fake_owner = Identity::generate();
        assert_ne!(real_owner.address(), fake_owner.address());
        let mut invite = Invite::issue(
            &real_owner,
            "mesh_01HX",
            "acme",
            "wss://broker.example/ws",
            2_000_000_000,
            Role::Member,
        );
        invite.owner = fake_owner.address();
        let err = invite.verify(1_700_000_000).unwrap_err();
        assert!(matches!(err, Error::BadSignature));
    }

    #[test]
    fn admin_and_member_signatures_differ() {
        let owner = Identity::generate();
        let m = Invite::issue(&owner, "mid", "s", "wss://b/", 2_000_000_000, Role::Member);
        let a = Invite::issue(&owner, "mid", "s", "wss://b/", 2_000_000_000, Role::Admin);
        assert_ne!(m.signature, a.signature);
    }

    #[test]
    fn json_round_trip() {
        let owner = Identity::generate();
        let invite = Invite::issue(
            &owner,
            "mesh_01HX",
            "acme",
            "wss://broker.example/ws",
            2_000_000_000,
            Role::Member,
        );
        let json = serde_json::to_string(&invite).unwrap();
        let back: Invite = serde_json::from_str(&json).unwrap();
        back.verify(1_700_000_000).expect("verify after round-trip");
    }
}
