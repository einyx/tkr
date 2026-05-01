//! EIP-55 checksummed Ethereum addresses, used as on-mesh peer identifiers.

use crate::{Error, Result};
use sha3::{Digest, Keccak256};
use std::fmt;
use std::str::FromStr;

/// 20-byte Ethereum address. Display impl uses EIP-55 mixed-case checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Address([u8; 20]);

impl Address {
    pub fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// EIP-55 checksum encoding. Returns "0x" + 40 mixed-case hex chars.
    pub fn to_checksum(&self) -> String {
        let lower = hex::encode(self.0);
        let hash = Keccak256::digest(lower.as_bytes());
        let mut out = String::with_capacity(42);
        out.push_str("0x");
        for (i, c) in lower.chars().enumerate() {
            if c.is_ascii_digit() {
                out.push(c);
            } else {
                let nibble = hash[i / 2] >> (4 * (1 - (i % 2))) & 0xf;
                if nibble >= 8 {
                    out.push(c.to_ascii_uppercase());
                } else {
                    out.push(c);
                }
            }
        }
        out
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_checksum())
    }
}

impl FromStr for Address {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let stripped = s.strip_prefix("0x").unwrap_or(s);
        if stripped.len() != 40 {
            return Err(Error::InvalidAddress(format!(
                "expected 40 hex chars, got {}",
                stripped.len()
            )));
        }
        let bytes = hex::decode(stripped)
            .map_err(|e| Error::InvalidAddress(format!("hex decode: {e}")))?;
        let mut out = [0u8; 20];
        out.copy_from_slice(&bytes);
        let addr = Address(out);

        // If the input had any uppercase hex chars, it's a checksum address
        // and MUST round-trip exactly. All-lowercase is also valid (no checksum).
        let has_mixed_case = stripped
            .chars()
            .any(|c| c.is_ascii_uppercase() && c.is_ascii_alphabetic());
        if has_mixed_case && stripped != &addr.to_checksum()[2..] {
            return Err(Error::InvalidAddress(
                "EIP-55 checksum mismatch".to_string(),
            ));
        }
        Ok(addr)
    }
}

impl serde::Serialize for Address {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_checksum())
    }
}

impl<'de> serde::Deserialize<'de> for Address {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // EIP-55 reference vectors from the spec.
    const EIP55_VECTORS: &[&str] = &[
        "0x52908400098527886E0F7030069857D2E4169EE7",
        "0x8617E340B3D01FA5F11F306F4090FD50E238070D",
        "0xde709f2102306220921060314715629080e2fb77",
        "0x27b1fdb04752bbc536007a920d24acb045561c26",
        "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
        "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
        "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
        "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
    ];

    #[test]
    fn eip55_round_trip() {
        for s in EIP55_VECTORS {
            let addr: Address = s.parse().expect(s);
            assert_eq!(addr.to_checksum(), *s);
        }
    }

    #[test]
    fn lowercase_accepted_no_checksum() {
        // All-lowercase is valid; uppercase-only would also be accepted
        // since there's no mixed-case to validate.
        let addr: Address = "0xde709f2102306220921060314715629080e2fb77"
            .parse()
            .unwrap();
        assert_eq!(
            addr.to_checksum(),
            "0xde709f2102306220921060314715629080e2fb77"
        );
    }

    #[test]
    fn mixed_case_wrong_checksum_rejected() {
        // Real address, but flip one nibble's case.
        let bad = "0x52908400098527886E0F7030069857d2E4169EE7";
        assert!(bad.parse::<Address>().is_err());
    }

    #[test]
    fn wrong_length_rejected() {
        assert!("0xdeadbeef".parse::<Address>().is_err());
        assert!("not-an-address".parse::<Address>().is_err());
    }

    #[test]
    fn serde_round_trip() {
        let addr: Address = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
            .parse()
            .unwrap();
        let json = serde_json::to_string(&addr).unwrap();
        assert_eq!(json, "\"0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed\"");
        let back: Address = serde_json::from_str(&json).unwrap();
        assert_eq!(back, addr);
    }
}
