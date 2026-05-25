//! Model manifest — the content-addressed description of a downloadable model.
//!
//! A manifest is a JSON document that lists every file the runner needs (weights,
//! tokenizer, configs) along with a CID for each file. The manifest itself is
//! hashed → the resulting CID is what registry entries point at.
//!
//! Stability matters: once a manifest is published, peers will gossip its CID and
//! cache it forever. Adding fields later is fine; renaming or removing them
//! breaks every previously-published manifest. Treat this struct like an on-wire
//! protocol, not an internal representation.
//!
//! # Hashing
//!
//! [`Manifest::cid`] hashes the manifest's canonical JSON serialization with
//! SHA-256 and returns a hex string. This matches IPFS's raw-block CIDs closely
//! enough for our purposes — peers treat the string as opaque.
//!
//! # The field set is intentionally unfilled.
//!
//! See the TODO in [`Manifest`] below — that's where your design input is needed.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sha3::Keccak256;
use jkr_mesh::{Address, Identity};

/// Content identifier — for v0 we use hex-encoded SHA-256. When we wire iroh,
/// switch this to a `iroh::Hash` newtype and update the `Display` impl.
pub type Cid = String;

/// One file inside a model bundle (weights, tokenizer, config, …).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct File {
    /// Relative path inside the model directory (e.g. `"model.gguf"`).
    pub path: String,
    /// Content ID of the file's bytes.
    pub cid: Cid,
    /// Size in bytes — lets a puller show progress + reject manifests whose
    /// total size exceeds a configured cap before fetching anything.
    pub size: u64,
}

/// Supported runners. The puller routes a model to whichever runner matches
/// this field; unknown values are rejected by older clients (forward
/// compatibility = add new variants, never repurpose old ones).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Runtime {
    LlamaCpp,
    Mlx,
}

/// Quantization scheme. Kept as a separate field (not baked into `name`) so
/// callers can filter ("Q4 only") and so the same logical model can be
/// republished at different quants without name collisions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Quant {
    F16,
    Q8,
    Q5KM,
    Q4KM,
    Q4_0,
    Q3KM,
    Q2K,
}

/// Runtime parameters the runner needs to load the model correctly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunParams {
    /// Maximum context length the model was trained / fine-tuned for.
    pub context: u32,
    /// Chat template name (`"llama3"`, `"chatml"`, …) — runner looks this up.
    /// `None` means the model is base / completion-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template: Option<String>,
    /// Optional rope frequency base — only set when overriding the GGUF default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rope_freq_base: Option<f32>,
}

/// A model manifest.
///
/// Stable on-wire shape — see module-level docs for the compatibility contract.
/// The `signature` field is excluded from [`Manifest::cid`] so publishers can
/// sign-then-hash without circular dependency.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Human handle, lowercase, e.g. `"llama-3.1-8b-instruct"`. Quant is *not*
    /// baked in; one name can have many quants under different manifests.
    pub name: String,
    /// Publisher-chosen version (date or semver). `(name, version, quant)`
    /// uniquely identifies a manifest within a publisher's namespace.
    pub version: String,
    /// Quantization scheme of the weight files in this manifest.
    pub quant: Quant,
    /// Every blob the runner needs (weights, tokenizer, configs).
    pub files: Vec<File>,
    /// Which runner loads this manifest.
    pub runtime: Runtime,
    /// Context length, chat template, rope overrides.
    pub params: RunParams,
    /// SPDX identifier (`"apache-2.0"`, `"llama3"`, `"other"`). Required so
    /// re-seeders can decide whether they're allowed to redistribute.
    pub license: String,
    /// Where the upstream weights came from (HF repo URL, original release
    /// page). Provenance for the human reading `jkr model show`.
    pub source_url: String,
    /// secp256k1 address that signed this manifest. Re-uses mesh identity.
    pub publisher: Address,
    /// Hex (0x-prefixed) 65-byte signature over [`Manifest::signing_bytes`].
    /// Required: every manifest on the mesh is attributable.
    pub signature: String,
}

impl Manifest {
    /// Bytes the publisher signs (everything except `signature` itself).
    /// Sign-then-hash: the signature is *not* part of the CID, so verifying a
    /// downloaded manifest doesn't require re-encoding around the signature.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut clone = self.clone();
        clone.signature = String::new();
        serde_json::to_vec(&clone).expect("Manifest serializes")
    }

    /// SHA-256 of the full manifest (including signature) → hex CID. This is
    /// what the gossip frame and registry index store, and what peers fetch by.
    pub fn cid(&self) -> Cid {
        let bytes = serde_json::to_vec(self).expect("Manifest serializes");
        hex::encode(Sha256::digest(&bytes))
    }

    /// Keccak-256 digest of [`Self::signing_bytes`] — the 32-byte digest the
    /// publisher signs. Keccak (not SHA-256) so we share `sign_digest` with
    /// the rest of the mesh (Hello, invites, envelopes all use Keccak).
    fn signing_digest(&self) -> [u8; 32] {
        let bytes = self.signing_bytes();
        Keccak256::digest(&bytes).into()
    }

    /// Sign this manifest in place. Sets `publisher` to `identity.address()`
    /// and writes the resulting 65-byte signature into `signature` as
    /// `0x`-prefixed hex.
    pub fn sign(&mut self, identity: &Identity) {
        self.publisher = identity.address();
        // Re-compute the digest *after* setting publisher so the signature
        // covers the final published address.
        let digest = self.signing_digest();
        let sig = identity.sign_digest(&digest);
        self.signature = format!("0x{}", hex::encode(sig));
    }

    /// Verify the signature recovers to the embedded `publisher` address.
    /// Returns the manifest's CID on success — callers usually want it.
    pub fn verify(&self) -> Result<Cid, ManifestError> {
        let sig = parse_eth_signature(&self.signature)?;
        let digest = self.signing_digest();
        let recovered = jkr_mesh::identity::recover_address(&digest, &sig)
            .map_err(|e| ManifestError::Crypto(e.to_string()))?;
        if recovered != self.publisher {
            return Err(ManifestError::SignatureMismatch);
        }
        Ok(self.cid())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("signature does not recover to the embedded publisher")]
    SignatureMismatch,
    #[error("malformed signature: {0}")]
    Encoding(String),
    #[error("crypto error: {0}")]
    Crypto(String),
}

fn parse_eth_signature(s: &str) -> Result<[u8; 65], ManifestError> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped)
        .map_err(|e| ManifestError::Encoding(format!("hex: {e}")))?;
    if bytes.len() != 65 {
        return Err(ManifestError::Encoding(format!(
            "expected 65 bytes, got {}",
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

    fn sample() -> Manifest {
        Manifest {
            name: "llama-3.1-8b-instruct".into(),
            version: "2026.05.02".into(),
            quant: Quant::Q4KM,
            files: vec![File {
                path: "model.gguf".into(),
                cid: "deadbeef".into(),
                size: 4_920_000_000,
            }],
            runtime: Runtime::LlamaCpp,
            params: RunParams {
                context: 8192,
                chat_template: Some("llama3".into()),
                rope_freq_base: None,
            },
            license: "llama3".into(),
            source_url: "https://huggingface.co/meta-llama/Llama-3.1-8B-Instruct".into(),
            publisher: Address::from_bytes([0u8; 20]),
            signature: "0x00".into(),
        }
    }

    #[test]
    fn manifest_roundtrips_through_json() {
        let m = sample();
        let bytes = serde_json::to_vec(&m).unwrap();
        let back: Manifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn signing_bytes_excludes_signature() {
        let mut m = sample();
        let with_sig_a = m.signing_bytes();
        m.signature = "0xffff".into();
        let with_sig_b = m.signing_bytes();
        assert_eq!(with_sig_a, with_sig_b, "signature must not affect signing bytes");
    }

    #[test]
    fn cid_changes_when_any_signed_field_changes() {
        let m = sample();
        let cid_a = m.cid();
        let mut m2 = m.clone();
        m2.version = "2026.05.03".into();
        assert_ne!(cid_a, m2.cid());
    }

    #[test]
    fn sign_then_verify_round_trips() {
        let id = Identity::generate();
        let mut m = sample();
        m.sign(&id);
        assert_eq!(m.publisher, id.address());
        m.verify().expect("freshly signed manifest verifies");
    }

    #[test]
    fn tampering_with_a_signed_field_breaks_verification() {
        let id = Identity::generate();
        let mut m = sample();
        m.sign(&id);
        m.version = "tampered".into();
        assert!(m.verify().is_err());
    }

    #[test]
    fn swapping_publisher_breaks_verification() {
        let id = Identity::generate();
        let other = Identity::generate();
        let mut m = sample();
        m.sign(&id);
        m.publisher = other.address();
        assert!(matches!(
            m.verify(),
            Err(ManifestError::SignatureMismatch)
        ));
    }
}
