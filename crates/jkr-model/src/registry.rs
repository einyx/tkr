//! Mesh-gossip model registry.
//!
//! Peers announce models they're seeding via signed [`ModelAnnounce`] messages.
//! Other peers cache `(publisher, name) → (version, manifest_cid, …)` tuples
//! and use them to resolve `jkr model pull <name>`.
//!
//! ## Design
//!
//! - **Self-contained payload.** `ModelAnnounce` carries its own signature so
//!   it doesn't depend on a particular mesh frame type. When the gossip frame
//!   lands in `jkr-mesh`, an announce becomes the `payload` field; until then,
//!   announces can be exchanged out-of-band (CLI, file drop, test fixtures).
//!
//! - **Resolution is ambiguous by design.** `resolve(name)` returns *every*
//!   publisher's claim. Trust ("which publisher do I treat as canonical for
//!   `llama-3.1-8b`") is the caller's problem — usually configured per-user.
//!   This is the only way a mesh registry can avoid name squatting without a
//!   central authority.
//!
//! - **Newer wins per (publisher, name).** A publisher republishing `llama3`
//!   at a new version supersedes their previous announce in the cache.
//!   Different publishers do not supersede each other.

use crate::manifest::Cid;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use jkr_mesh::{Address, Identity};

/// Signed gossip message: "publisher P is seeding model `name@version`,
/// manifest CID is `manifest_cid`."
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModelAnnounce {
    pub name: String,
    pub version: String,
    pub manifest_cid: Cid,
    pub publisher: Address,
    /// Unix milliseconds. Lets caches reject replays of stale announces and
    /// pick the newest claim from a single publisher.
    pub timestamp_ms: u64,
    /// `0x`-prefixed hex of a 65-byte secp256k1 signature over
    /// `keccak256(name || 0x00 || version || 0x00 || manifest_cid || 0x00 || publisher || ts)`.
    pub signature: String,
}

impl ModelAnnounce {
    /// Build and sign an announce. The signature covers all other fields.
    pub fn signed(
        identity: &Identity,
        name: impl Into<String>,
        version: impl Into<String>,
        manifest_cid: impl Into<Cid>,
        timestamp_ms: u64,
    ) -> Self {
        let name = name.into();
        let version = version.into();
        let manifest_cid = manifest_cid.into();
        let publisher = identity.address();
        let digest = announce_digest(&name, &version, &manifest_cid, &publisher, timestamp_ms);
        let sig = identity.sign_digest(&digest);
        Self {
            name,
            version,
            manifest_cid,
            publisher,
            timestamp_ms,
            signature: format!("0x{}", hex::encode(sig)),
        }
    }

    /// Verify the signature recovers to `publisher`.
    pub fn verify(&self) -> Result<(), AnnounceError> {
        let sig = parse_signature(&self.signature)?;
        let digest = announce_digest(
            &self.name,
            &self.version,
            &self.manifest_cid,
            &self.publisher,
            self.timestamp_ms,
        );
        let recovered = jkr_mesh::identity::recover_address(&digest, &sig)
            .map_err(|e| AnnounceError::Crypto(e.to_string()))?;
        if recovered != self.publisher {
            return Err(AnnounceError::SignatureMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AnnounceError {
    #[error("signature does not recover to publisher")]
    SignatureMismatch,
    #[error("malformed signature: {0}")]
    Encoding(String),
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("announce timestamp is outside the freshness window")]
    Stale,
    #[error("announce supersedes is older than cached entry")]
    Older,
}

fn announce_digest(
    name: &str,
    version: &str,
    manifest_cid: &str,
    publisher: &Address,
    timestamp_ms: u64,
) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(name.as_bytes());
    h.update([0u8]);
    h.update(version.as_bytes());
    h.update([0u8]);
    h.update(manifest_cid.as_bytes());
    h.update([0u8]);
    h.update(publisher.as_bytes());
    h.update(timestamp_ms.to_be_bytes());
    h.finalize().into()
}

fn parse_signature(s: &str) -> Result<[u8; 65], AnnounceError> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped).map_err(|e| AnnounceError::Encoding(format!("hex: {e}")))?;
    if bytes.len() != 65 {
        return Err(AnnounceError::Encoding(format!(
            "expected 65 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 65];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// In-memory cache of announces, keyed by `(publisher, name)`.
///
/// Per-publisher dedup means a single peer republishing supersedes their own
/// previous announce, but never another peer's claim. Resolution is ambiguous
/// on purpose — see module docs.
pub struct RegistryCache {
    /// Max age for an announce before [`RegistryCache::ingest`] rejects it.
    /// Caches discard stale entries lazily on `resolve`.
    max_age: Duration,
    entries: HashMap<(Address, String), ModelAnnounce>,
}

impl RegistryCache {
    pub fn new(max_age: Duration) -> Self {
        Self {
            max_age,
            entries: HashMap::new(),
        }
    }

    /// Verify and insert an announce. Rejects stale (older than `max_age`),
    /// future-dated (>60s ahead of `now_ms`), or signature-invalid messages.
    /// Rejects announces older than what we already have for this publisher.
    pub fn ingest(&mut self, announce: ModelAnnounce, now_ms: u64) -> Result<(), AnnounceError> {
        announce.verify()?;
        let age = now_ms.saturating_sub(announce.timestamp_ms);
        let future_skew = announce.timestamp_ms.saturating_sub(now_ms);
        if age > self.max_age.as_millis() as u64 || future_skew > 60_000 {
            return Err(AnnounceError::Stale);
        }
        let key = (announce.publisher, announce.name.clone());
        if let Some(existing) = self.entries.get(&key) {
            if existing.timestamp_ms >= announce.timestamp_ms {
                return Err(AnnounceError::Older);
            }
        }
        self.entries.insert(key, announce);
        Ok(())
    }

    /// Every publisher's current claim for `name`. Caller picks which to trust.
    /// Stale entries (older than `max_age` relative to `now_ms`) are filtered
    /// out and *not* removed — call [`RegistryCache::sweep`] to free memory.
    pub fn resolve(&self, name: &str, now_ms: u64) -> Vec<&ModelAnnounce> {
        let cutoff = self.max_age.as_millis() as u64;
        self.entries
            .values()
            .filter(|a| a.name == name && now_ms.saturating_sub(a.timestamp_ms) <= cutoff)
            .collect()
    }

    /// Drop every entry older than `max_age` relative to `now_ms`.
    pub fn sweep(&mut self, now_ms: u64) {
        let cutoff = self.max_age.as_millis() as u64;
        self.entries
            .retain(|_, a| now_ms.saturating_sub(a.timestamp_ms) <= cutoff);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Convenience: current wall-clock millis. Tests pass explicit times instead.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_announce(id: &Identity, name: &str, ts: u64) -> ModelAnnounce {
        ModelAnnounce::signed(id, name, "v1", "deadbeef", ts)
    }

    #[test]
    fn signed_announce_verifies() {
        let id = Identity::generate();
        let a = signed_announce(&id, "llama3", 1_000_000);
        assert!(a.verify().is_ok());
    }

    #[test]
    fn tampered_announce_fails_verification() {
        let id = Identity::generate();
        let mut a = signed_announce(&id, "llama3", 1_000_000);
        a.version = "tampered".into();
        assert!(a.verify().is_err());
    }

    #[test]
    fn ingest_rejects_stale() {
        let id = Identity::generate();
        let mut cache = RegistryCache::new(Duration::from_secs(60));
        let a = signed_announce(&id, "llama3", 1_000_000);
        // now is 10 minutes after the announce → stale (max_age = 60s).
        let err = cache.ingest(a, 1_000_000 + 600_000).unwrap_err();
        assert!(matches!(err, AnnounceError::Stale));
    }

    #[test]
    fn ingest_rejects_future_skew() {
        let id = Identity::generate();
        let mut cache = RegistryCache::new(Duration::from_secs(60));
        // announce timestamped 5 minutes in the future.
        let a = signed_announce(&id, "llama3", 1_000_000 + 300_000);
        let err = cache.ingest(a, 1_000_000).unwrap_err();
        assert!(matches!(err, AnnounceError::Stale));
    }

    #[test]
    fn newer_announce_supersedes_older_from_same_publisher() {
        let id = Identity::generate();
        let mut cache = RegistryCache::new(Duration::from_secs(3600));
        let a1 = ModelAnnounce::signed(&id, "llama3", "v1", "cid-a", 1_000_000);
        let a2 = ModelAnnounce::signed(&id, "llama3", "v2", "cid-b", 1_000_500);
        cache.ingest(a1, 1_000_500).unwrap();
        cache.ingest(a2.clone(), 1_000_500).unwrap();
        let resolved = cache.resolve("llama3", 1_000_500);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0], &a2);
    }

    #[test]
    fn ingest_rejects_older_replay_from_same_publisher() {
        let id = Identity::generate();
        let mut cache = RegistryCache::new(Duration::from_secs(3600));
        let new = ModelAnnounce::signed(&id, "llama3", "v2", "cid-b", 1_000_500);
        let old = ModelAnnounce::signed(&id, "llama3", "v1", "cid-a", 1_000_000);
        cache.ingest(new, 1_000_500).unwrap();
        let err = cache.ingest(old, 1_000_500).unwrap_err();
        assert!(matches!(err, AnnounceError::Older));
    }

    #[test]
    fn resolve_returns_all_publishers_claims() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let mut cache = RegistryCache::new(Duration::from_secs(3600));
        cache
            .ingest(signed_announce(&alice, "llama3", 1_000_000), 1_000_500)
            .unwrap();
        cache
            .ingest(signed_announce(&bob, "llama3", 1_000_100), 1_000_500)
            .unwrap();
        let resolved = cache.resolve("llama3", 1_000_500);
        assert_eq!(resolved.len(), 2, "both publishers' claims must surface");
    }

    #[test]
    fn sweep_drops_stale_entries() {
        let id = Identity::generate();
        let mut cache = RegistryCache::new(Duration::from_secs(60));
        cache
            .ingest(signed_announce(&id, "llama3", 1_000_000), 1_000_000)
            .unwrap();
        assert_eq!(cache.len(), 1);
        cache.sweep(1_000_000 + 120_000);
        assert!(cache.is_empty());
    }
}
