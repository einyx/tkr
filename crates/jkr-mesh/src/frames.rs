//! Wire frames for the broker ↔ client protocol.
//!
//! All frames are JSON objects with a mandatory `type` field. Unknown
//! frames are ignored silently by both ends. Field names are camelCase
//! to match the eventual JS/TS clients we expect to interoperate with.

use crate::{Address, Envelope, Error, Identity, Result};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

// ---------- Hello ----------

/// Client → broker, first frame after WS open. Authenticates the
/// connection by signing `(meshId, address, sessionId, timestamp)` with
/// the member's identity key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    #[serde(rename = "type")]
    pub kind: HelloTag,
    pub mesh_id: String,
    pub address: Address,
    pub session_id: String,
    pub timestamp_ms: u64,
    /// 65-byte Ethereum signature, hex.
    pub signature: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HelloTag {
    #[serde(rename = "hello")]
    Hello,
}

impl Hello {
    pub fn new(
        identity: &Identity,
        mesh_id: impl Into<String>,
        session_id: impl Into<String>,
        timestamp_ms: u64,
    ) -> Self {
        let mesh_id = mesh_id.into();
        let session_id = session_id.into();
        let digest = hello_digest(&mesh_id, &identity.address(), &session_id, timestamp_ms);
        let sig = identity.sign_digest(&digest);
        Self {
            kind: HelloTag::Hello,
            mesh_id,
            address: identity.address(),
            session_id,
            timestamp_ms,
            signature: format!("0x{}", hex::encode(sig)),
        }
    }

    /// Verify the hello signature against the embedded `address`. The
    /// broker calls this on receive; the result tells it which member
    /// claims this connection (and the signature proves they hold the
    /// matching key).
    ///
    /// **Does not check freshness** — the broker should call
    /// [`Hello::verify_with_now`] instead, so a captured Hello cannot be
    /// replayed by a network adversary at a later time.
    pub fn verify(&self) -> Result<()> {
        let digest = hello_digest(&self.mesh_id, &self.address, &self.session_id, self.timestamp_ms);
        let sig_bytes = parse_signature(&self.signature)?;
        let recovered = crate::identity::recover_address(&digest, &sig_bytes)?;
        if recovered != self.address {
            return Err(Error::BadSignature);
        }
        Ok(())
    }

    /// Verify the signature **and** check that the embedded `timestamp_ms`
    /// is within `max_skew_ms` of `now_ms` (in either direction). Brokers
    /// should call this with a small skew (e.g. 60_000 ms) so a captured
    /// Hello frame cannot be replayed by a network adversary later.
    pub fn verify_with_now(&self, now_ms: u64, max_skew_ms: u64) -> Result<()> {
        self.verify()?;
        let skew = self.timestamp_ms.abs_diff(now_ms);
        if skew > max_skew_ms {
            return Err(Error::Encoding(format!(
                "hello timestamp out of window: skew={skew}ms, max={max_skew_ms}ms"
            )));
        }
        Ok(())
    }
}

/// Default freshness window the broker enforces against `Hello.timestamp_ms`.
/// Tuned for typical clock skew between client and broker.
pub const HELLO_MAX_SKEW_MS: u64 = 60_000;

fn hello_digest(mesh_id: &str, addr: &Address, session_id: &str, ts: u64) -> [u8; 32] {
    // Domain-tagged keccak so the hello signature can never be confused
    // with an EIP-712 invite or a payment receipt.
    let mut h = Keccak256::new();
    h.update(b"jkr-mesh/v1/hello\n");
    h.update(mesh_id.as_bytes());
    h.update(b"\n");
    h.update(addr.as_bytes());
    h.update(b"\n");
    h.update(session_id.as_bytes());
    h.update(b"\n");
    h.update(ts.to_be_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

// ---------- Send ----------

/// Client → broker, encrypted DM directed at another member.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Send {
    #[serde(rename = "type")]
    pub kind: SendTag,
    /// 16-hex correlation id chosen by sender.
    pub id: String,
    pub to: Address,
    pub priority: Priority,
    pub envelope: Envelope,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SendTag {
    #[serde(rename = "send")]
    Send,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Now,
    Next,
    Low,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Next
    }
}

// ---------- Push ----------

/// Broker → client, delivers a `Send` from another member. `from` is the
/// sender's verified mesh address (broker authenticated their hello).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Push {
    #[serde(rename = "type")]
    pub kind: PushTag,
    pub id: String,
    pub from: Address,
    pub envelope: Envelope,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PushTag {
    #[serde(rename = "push")]
    Push,
}

// ---------- Ack / Error ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    #[serde(rename = "type")]
    pub kind: AckTag,
    /// Echoes the `id` of the frame being ack'd (Send or Hello).
    pub id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AckTag {
    #[serde(rename = "ack")]
    Ack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorFrame {
    #[serde(rename = "type")]
    pub kind: ErrorTag,
    pub code: String,
    pub message: String,
    /// Optional id correlating the error to a prior frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorTag {
    #[serde(rename = "error")]
    Error,
}

// ---------- Frame enum (untagged dispatch) ----------

/// Tagged-union of every frame type. Decoded by `serde`'s
/// `#[serde(tag = "type")]` so the wire stays single-object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Frame {
    Hello(HelloFields),
    Ack(AckFields),
    Send(SendFields),
    Push(PushFields),
    Error(ErrorFields),
}

// To make serde's `tag = "type"` work alongside the standalone Hello/etc.
// types above (which include their own `kind` for ergonomic construction),
// the Frame enum uses parallel "fields-only" structs without the redundant tag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HelloFields {
    pub mesh_id: String,
    pub address: Address,
    pub session_id: String,
    pub timestamp_ms: u64,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AckFields {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SendFields {
    pub id: String,
    pub to: Address,
    #[serde(default)]
    pub priority: Priority,
    pub envelope: Envelope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PushFields {
    pub id: String,
    pub from: Address,
    pub envelope: Envelope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorFields {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl From<Hello> for Frame {
    fn from(h: Hello) -> Self {
        Frame::Hello(HelloFields {
            mesh_id: h.mesh_id,
            address: h.address,
            session_id: h.session_id,
            timestamp_ms: h.timestamp_ms,
            signature: h.signature,
        })
    }
}

impl Frame {
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| Error::Encoding(format!("frame encode: {e}")))
    }

    pub fn from_json(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|e| Error::Encoding(format!("frame decode: {e}")))
    }
}

// ---------- helpers ----------

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
    use k256::SecretKey;

    fn pubkey_of(id: &Identity) -> k256::PublicKey {
        let sk = SecretKey::from_slice(&id.secret_bytes()).unwrap();
        sk.public_key()
    }

    #[test]
    fn hello_signs_and_verifies() {
        let id = Identity::generate();
        let hello = Hello::new(&id, "mesh_x", "sess-1", 1_700_000_000_000);
        hello.verify().unwrap();
    }

    #[test]
    fn hello_tampered_field_fails() {
        let id = Identity::generate();
        let mut hello = Hello::new(&id, "mesh_x", "sess-1", 1_700_000_000_000);
        hello.session_id = "evil".to_string();
        assert!(hello.verify().is_err());
    }

    #[test]
    fn hello_swapped_address_fails() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let mut hello = Hello::new(&alice, "mesh_x", "sess-1", 1_700_000_000_000);
        hello.address = bob.address();
        assert!(hello.verify().is_err());
    }

    #[test]
    fn frame_round_trips_send() {
        let bob = Identity::generate();
        let env = Envelope::seal(b"ping", &pubkey_of(&bob), bob.address()).unwrap();
        let frame = Frame::Send(SendFields {
            id: "0123456789abcdef".to_string(),
            to: bob.address(),
            priority: Priority::Next,
            envelope: env,
        });
        let json = frame.to_json().unwrap();
        let back = Frame::from_json(&json).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn frame_round_trips_each_variant() {
        let id = Identity::generate();
        let bob = Identity::generate();
        let env = Envelope::seal(b"x", &pubkey_of(&bob), bob.address()).unwrap();
        let frames = vec![
            Frame::from(Hello::new(&id, "m", "s", 1)),
            Frame::Ack(AckFields {
                id: "abc".into(),
            }),
            Frame::Send(SendFields {
                id: "id1".into(),
                to: bob.address(),
                priority: Priority::Now,
                envelope: env.clone(),
            }),
            Frame::Push(PushFields {
                id: "id1".into(),
                from: id.address(),
                envelope: env,
            }),
            Frame::Error(ErrorFields {
                code: "rate_limited".into(),
                message: "slow down".into(),
                id: Some("id1".into()),
            }),
        ];
        for f in &frames {
            let json = f.to_json().unwrap();
            let back = Frame::from_json(&json).unwrap();
            assert_eq!(&back, f, "frame round-trip mismatch: {json}");
        }
    }

    #[test]
    fn unknown_frame_type_errors() {
        let json = r#"{"type":"banana","x":1}"#;
        assert!(Frame::from_json(json).is_err());
    }

    #[test]
    fn priority_default_is_next() {
        // Send without priority field should default to Next.
        let bob = Identity::generate();
        let env = Envelope::seal(b"x", &pubkey_of(&bob), bob.address()).unwrap();
        let env_json = serde_json::to_string(&env).unwrap();
        let json = format!(
            r#"{{"type":"send","id":"abc","to":"{}","envelope":{}}}"#,
            bob.address(),
            env_json
        );
        let back = Frame::from_json(&json).unwrap();
        if let Frame::Send(s) = back {
            assert_eq!(s.priority, Priority::Next);
        } else {
            panic!("expected Send frame");
        }
    }

    #[test]
    fn json_uses_camel_case() {
        // Wire field names must be camelCase, not snake_case.
        let id = Identity::generate();
        let hello = Hello::new(&id, "m", "s", 1);
        let frame: Frame = hello.into();
        let json = frame.to_json().unwrap();
        assert!(json.contains("\"meshId\""), "expected camelCase: {json}");
        assert!(json.contains("\"sessionId\""));
        assert!(json.contains("\"timestampMs\""));
        assert!(!json.contains("mesh_id"));
    }
}
