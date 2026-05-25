//! Mesh invites. EIP-712 typed-data signed by the mesh owner; renders
//! human-readably in any wallet that supports `eth_signTypedData_v4`.

use crate::{identity::recover_address, Address, Error, Identity, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
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

    /// Encode the invite as a base64url token. Used inside `to_url()` and
    /// also accepted on its own by `parse_url()` for paste-friendly invites.
    pub fn to_token(&self) -> Result<String> {
        let json = serde_json::to_vec(self)
            .map_err(|e| Error::Encoding(format!("invite serialize: {e}")))?;
        Ok(B64URL.encode(json))
    }

    /// Wrap the invite into a URL. `host_url` is anything containing a
    /// host the join page is served from — typically the broker URL with
    /// `wss://` swapped for `https://`. The path `/join/<token>` is appended.
    pub fn to_url(&self, host_url: &str) -> Result<String> {
        let host = host_url
            .trim_end_matches('/')
            .trim_start_matches("ws://")
            .trim_start_matches("wss://")
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        let host = host.split('/').next().unwrap_or(host);
        if host.is_empty() {
            return Err(Error::Encoding("empty host_url".to_string()));
        }
        Ok(format!("https://{host}/join/{}", self.to_token()?))
    }

    /// Parse an invite from any of the accepted URL shapes:
    /// - `https://<host>/join/<token>`
    /// - `https://<host>/<locale>/join/<token>` (two-letter locale prefix)
    /// - `jkrmesh://join/<token>`
    /// - bare base64url token (≥ 20 chars, alphabet `[A-Za-z0-9_-]`)
    ///
    /// This *parses* the structure but does **not** verify the signature
    /// or expiry — callers must subsequently call [`Invite::verify`].
    pub fn parse_url(s: &str) -> Result<Self> {
        let token = extract_token(s.trim())?;
        let json = B64URL
            .decode(&token)
            .map_err(|e| Error::InvalidInvite(format!("base64url decode: {e}")))?;
        let invite: Invite = serde_json::from_slice(&json)
            .map_err(|e| Error::InvalidInvite(format!("json parse: {e}")))?;
        Ok(invite)
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

/// EIP-712 domain for invites. Pinned constants — changing any field
/// invalidates all previously-issued invites.
///
/// Shape matches the standard `EIP712Domain(string name,string version,
/// uint256 chainId,address verifyingContract)` so wallets render it
/// consistently with the on-chain `MeshEscrow` receipt domain. We use
/// `chainId = 0` and `verifyingContract = address(0)` because invites are
/// purely off-chain — the deployment binding lives in the struct hash via
/// `brokerUrl` (and on the broker side, via the enrolled mesh_id record).
/// Including the zero values explicitly still adds defence-in-depth: a
/// future on-chain invite registry would use a non-zero pair, eliminating
/// any structural ambiguity between off-chain and on-chain invites.
fn domain_separator() -> [u8; 32] {
    let type_hash = Keccak256::digest(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
    );
    let mut buf = Vec::with_capacity(5 * 32);
    buf.extend_from_slice(&type_hash);
    buf.extend_from_slice(&Keccak256::digest(b"jkr-mesh"));
    buf.extend_from_slice(&Keccak256::digest(b"1"));
    // chainId = 0 (off-chain)
    buf.extend_from_slice(&[0u8; 32]);
    // verifyingContract = address(0)
    buf.extend_from_slice(&[0u8; 32]);
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

fn extract_token(s: &str) -> Result<String> {
    // jkrmesh://join/<token>
    if let Some(rest) = s.strip_prefix("jkrmesh://join/") {
        return validate_bare_token(rest);
    }

    // https?://<host>[/locale]/join/<token>
    let after_scheme = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    if let Some(idx) = after_scheme.find("/join/") {
        let token = &after_scheme[idx + "/join/".len()..];
        return validate_bare_token(token);
    }

    // bare token
    validate_bare_token(s)
}

fn validate_bare_token(s: &str) -> Result<String> {
    let token = s.split(|c: char| c == '?' || c == '#' || c == '/').next().unwrap_or(s);
    if token.len() < 20 {
        return Err(Error::InvalidInvite(format!(
            "token too short ({} chars)",
            token.len()
        )));
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Error::InvalidInvite(
            "token contains non-base64url chars".to_string(),
        ));
    }
    Ok(token.to_string())
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

    fn sample_invite() -> Invite {
        Invite::issue(
            &Identity::generate(),
            "mesh_01HX",
            "acme",
            "wss://broker.example/ws",
            2_000_000_000,
            Role::Member,
        )
    }

    #[test]
    fn url_round_trip_https() {
        let invite = sample_invite();
        let url = invite.to_url("wss://broker.example/ws").unwrap();
        assert!(url.starts_with("https://broker.example/join/"));
        let back = Invite::parse_url(&url).unwrap();
        back.verify(1_700_000_000).unwrap();
        assert_eq!(back.signature, invite.signature);
    }

    #[test]
    fn url_round_trip_with_locale_prefix() {
        let invite = sample_invite();
        let token = invite.to_token().unwrap();
        let url = format!("https://broker.example/en/join/{token}");
        let back = Invite::parse_url(&url).unwrap();
        assert_eq!(back.signature, invite.signature);
    }

    #[test]
    fn url_round_trip_custom_scheme() {
        let invite = sample_invite();
        let token = invite.to_token().unwrap();
        let url = format!("jkrmesh://join/{token}");
        let back = Invite::parse_url(&url).unwrap();
        assert_eq!(back.signature, invite.signature);
    }

    #[test]
    fn bare_token_accepted() {
        let invite = sample_invite();
        let token = invite.to_token().unwrap();
        let back = Invite::parse_url(&token).unwrap();
        assert_eq!(back.signature, invite.signature);
    }

    #[test]
    fn token_with_query_string_stripped() {
        let invite = sample_invite();
        let token = invite.to_token().unwrap();
        let url = format!("https://broker.example/join/{token}?utm_source=x");
        let back = Invite::parse_url(&url).unwrap();
        assert_eq!(back.signature, invite.signature);
    }

    #[test]
    fn short_token_rejected() {
        assert!(Invite::parse_url("short").is_err());
    }

    #[test]
    fn invalid_base64_rejected() {
        // 20+ chars, valid url alphabet, but not actually valid base64url JSON.
        let url = "https://broker.example/join/____invalid_token____";
        assert!(Invite::parse_url(url).is_err());
    }

    #[test]
    fn to_url_strips_wss_prefix() {
        let invite = sample_invite();
        let url = invite.to_url("wss://broker.example/ws").unwrap();
        assert!(url.starts_with("https://broker.example/join/"));
    }

    #[test]
    fn parsed_invite_then_tampered_fails_verify() {
        let invite = sample_invite();
        let url = invite.to_url("wss://broker.example/ws").unwrap();
        let mut back = Invite::parse_url(&url).unwrap();
        back.mesh_slug = "evil".to_string();
        assert!(back.verify(1_700_000_000).is_err());
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
