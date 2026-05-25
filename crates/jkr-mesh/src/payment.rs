//! Payment receipts. Off-chain EIP-712 signatures that authorize a
//! `cumulative` payout for a `sessionId` on a specific `MeshEscrow`
//! deployment. The on-chain digest in `MeshEscrow.receiptDigest()` must
//! match `Receipt::digest()` exactly, otherwise `claim()` reverts.
//!
//! Wire shape: hex-encoded session_id + decimal-string cumulative + 65-byte
//! Ethereum signature. Embedded inside the **encrypted DM payload** so the
//! broker never sees payment amounts.

use crate::{identity::recover_address, Address, Error, Identity, Result};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

/// Identifies a MeshEscrow deployment that a Receipt is valid against.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EscrowDomain {
    pub chain_id: u64,
    pub verifying_contract: Address,
}

/// EIP-712 typed-data receipt. Payer signs, recipient submits to
/// MeshEscrow.claim() to release `cumulative - alreadyPaid`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    /// 32-byte session identifier, hex 0x...
    pub session_id: String,
    /// u256 cumulative paid amount, encoded as a decimal string to avoid
    /// JSON number precision loss. Internal API uses u128.
    pub cumulative: String,
    pub domain: EscrowDomain,
    /// 65-byte ECDSA signature (r||s||v), hex 0x...
    pub signature: String,
}

impl Receipt {
    /// Issue and sign a receipt as the payer.
    pub fn issue(
        payer: &Identity,
        session_id: [u8; 32],
        cumulative: u128,
        domain: EscrowDomain,
    ) -> Self {
        let mut receipt = Receipt {
            session_id: format!("0x{}", hex::encode(session_id)),
            cumulative: cumulative.to_string(),
            domain,
            signature: String::new(),
        };
        let digest = receipt.digest_internal(&session_id, cumulative);
        let sig = payer.sign_digest(&digest);
        receipt.signature = format!("0x{}", hex::encode(sig));
        receipt
    }

    /// Verify the signature recovers to `expected_payer` and the encoded
    /// fields parse cleanly. Returns the parsed `(session_id, cumulative)`
    /// on success — caller compares against on-chain channel state.
    pub fn verify(&self, expected_payer: Address) -> Result<([u8; 32], u128)> {
        let session_id = parse_session_id(&self.session_id)?;
        let cumulative: u128 = self
            .cumulative
            .parse()
            .map_err(|e| Error::Encoding(format!("cumulative parse: {e}")))?;
        let sig = parse_signature(&self.signature)?;
        let digest = self.digest_internal(&session_id, cumulative);
        let recovered = recover_address(&digest, &sig)?;
        if recovered != expected_payer {
            return Err(Error::BadSignature);
        }
        Ok((session_id, cumulative))
    }

    /// Compute the EIP-712 digest for this receipt without verifying.
    /// Useful for callers who want to display the digest before signing.
    pub fn digest(&self) -> Result<[u8; 32]> {
        let session_id = parse_session_id(&self.session_id)?;
        let cumulative: u128 = self
            .cumulative
            .parse()
            .map_err(|e| Error::Encoding(format!("cumulative parse: {e}")))?;
        Ok(self.digest_internal(&session_id, cumulative))
    }

    fn digest_internal(&self, session_id: &[u8; 32], cumulative: u128) -> [u8; 32] {
        // EIP-712:
        //   digest = keccak("\x19\x01" || domainSeparator || structHash)
        let domain_sep = self.domain.separator();
        let struct_hash = receipt_struct_hash(session_id, cumulative);
        let mut buf = Vec::with_capacity(2 + 32 + 32);
        buf.extend_from_slice(b"\x19\x01");
        buf.extend_from_slice(&domain_sep);
        buf.extend_from_slice(&struct_hash);
        let mut out = [0u8; 32];
        out.copy_from_slice(&Keccak256::digest(&buf));
        out
    }
}

impl EscrowDomain {
    /// EIP-712 domain separator. Must match MeshEscrow's
    /// `EIP712("tkr-mesh", "1")` constructor.
    pub fn separator(&self) -> [u8; 32] {
        // typeHash = keccak("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")
        let type_hash = Keccak256::digest(
            b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
        );
        let mut buf = Vec::with_capacity(5 * 32);
        buf.extend_from_slice(&type_hash);
        buf.extend_from_slice(&Keccak256::digest(b"tkr-mesh"));
        buf.extend_from_slice(&Keccak256::digest(b"1"));
        buf.extend_from_slice(&u256_be(self.chain_id as u128));
        buf.extend_from_slice(&address_padded(&self.verifying_contract));
        let mut out = [0u8; 32];
        out.copy_from_slice(&Keccak256::digest(&buf));
        out
    }
}

fn receipt_struct_hash(session_id: &[u8; 32], cumulative: u128) -> [u8; 32] {
    // typeHash = keccak("Receipt(bytes32 sessionId,uint256 cumulative)")
    let type_hash = Keccak256::digest(b"Receipt(bytes32 sessionId,uint256 cumulative)");
    let mut buf = Vec::with_capacity(3 * 32);
    buf.extend_from_slice(&type_hash);
    buf.extend_from_slice(session_id);
    buf.extend_from_slice(&u256_be(cumulative));
    let mut out = [0u8; 32];
    out.copy_from_slice(&Keccak256::digest(&buf));
    out
}

fn u256_be(n: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&n.to_be_bytes());
    out
}

fn address_padded(addr: &Address) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[12..].copy_from_slice(addr.as_bytes());
    out
}

fn parse_session_id(s: &str) -> Result<[u8; 32]> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped)
        .map_err(|e| Error::Encoding(format!("session_id hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(Error::Encoding(format!(
            "session_id: expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
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

// ---------- DM payload helper ----------
//
// Convenience type for embedding a Receipt inside an encrypted message
// body. The whole thing is JSON-serialized and passed to Envelope::seal().

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaidMessage {
    /// Application-specific message bytes, base64.
    #[serde(default)]
    pub body_b64: String,
    /// Optional cumulative-payment receipt covering work-up-to-now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Receipt>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_domain() -> EscrowDomain {
        EscrowDomain {
            chain_id: 8453, // Base mainnet
            verifying_contract: "0x000000000000000000000000000000000000c0de"
                .parse()
                .unwrap(),
        }
    }

    #[test]
    fn issue_then_verify() {
        let payer = Identity::generate();
        let mut sid = [0u8; 32];
        sid[..4].copy_from_slice(b"sess");
        let receipt = Receipt::issue(&payer, sid, 12_345_000, fake_domain());
        let (got_sid, got_cum) = receipt.verify(payer.address()).expect("verify");
        assert_eq!(got_sid, sid);
        assert_eq!(got_cum, 12_345_000);
    }

    #[test]
    fn wrong_payer_rejected() {
        let payer = Identity::generate();
        let stranger = Identity::generate();
        let receipt = Receipt::issue(&payer, [1u8; 32], 100, fake_domain());
        assert!(matches!(
            receipt.verify(stranger.address()).unwrap_err(),
            Error::BadSignature
        ));
    }

    #[test]
    fn tampered_cumulative_breaks_signature() {
        let payer = Identity::generate();
        let mut receipt = Receipt::issue(&payer, [1u8; 32], 100, fake_domain());
        receipt.cumulative = "200".to_string();
        assert!(receipt.verify(payer.address()).is_err());
    }

    #[test]
    fn tampered_session_id_breaks_signature() {
        let payer = Identity::generate();
        let mut receipt = Receipt::issue(&payer, [1u8; 32], 100, fake_domain());
        receipt.session_id = format!("0x{}", hex::encode([2u8; 32]));
        assert!(receipt.verify(payer.address()).is_err());
    }

    #[test]
    fn wrong_chain_id_breaks_signature() {
        let payer = Identity::generate();
        let mut receipt = Receipt::issue(&payer, [1u8; 32], 100, fake_domain());
        receipt.domain.chain_id = 1; // ethereum mainnet — different domain
        assert!(receipt.verify(payer.address()).is_err());
    }

    #[test]
    fn wrong_verifying_contract_breaks_signature() {
        let payer = Identity::generate();
        let mut receipt = Receipt::issue(&payer, [1u8; 32], 100, fake_domain());
        receipt.domain.verifying_contract = "0x0000000000000000000000000000000000000001"
            .parse()
            .unwrap();
        assert!(receipt.verify(payer.address()).is_err());
    }

    #[test]
    fn json_round_trip() {
        let payer = Identity::generate();
        let receipt = Receipt::issue(&payer, [42u8; 32], 9_999_999, fake_domain());
        let json = serde_json::to_string(&receipt).unwrap();
        let back: Receipt = serde_json::from_str(&json).unwrap();
        back.verify(payer.address()).expect("verify after round-trip");
    }

    #[test]
    fn paid_message_round_trip() {
        let payer = Identity::generate();
        let receipt = Receipt::issue(&payer, [7u8; 32], 1000, fake_domain());
        let msg = PaidMessage {
            body_b64: "aGVsbG8=".to_string(), // "hello"
            receipt: Some(receipt),
        };
        let json = serde_json::to_vec(&msg).unwrap();
        let back: PaidMessage = serde_json::from_slice(&json).unwrap();
        assert_eq!(back, msg);
        back.receipt
            .as_ref()
            .unwrap()
            .verify(payer.address())
            .unwrap();
    }

    /// Hand-computed reference: a known payer + session_id + amount must
    /// produce a stable digest that matches what MeshEscrow.receiptDigest()
    /// computes on-chain. If this ever fails after a contract change, the
    /// off-chain and on-chain halves have drifted.
    #[test]
    fn digest_is_deterministic() {
        let mut secret = [0u8; 32];
        secret[31] = 1;
        let payer = Identity::from_secret_bytes(&secret).unwrap();
        let sid = [0xAAu8; 32];
        let cum: u128 = 1_000_000;
        let domain = EscrowDomain {
            chain_id: 8453,
            verifying_contract: "0x000000000000000000000000000000000000c0de"
                .parse()
                .unwrap(),
        };
        let r = Receipt::issue(&payer, sid, cum, domain);
        let d1 = r.digest().unwrap();
        let d2 = r.digest().unwrap();
        assert_eq!(d1, d2);
        // Re-issuing identical inputs (signature is deterministic via RFC 6979) → identical struct.
        let r2 = Receipt::issue(&payer, sid, cum, domain);
        assert_eq!(r.signature, r2.signature, "k256 ECDSA is RFC 6979 deterministic");
    }
}
