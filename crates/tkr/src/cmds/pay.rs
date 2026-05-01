//! `tkr pay` — agent-to-agent payments. Off-chain receipt issuance and
//! verification today; on-chain `claim` lands once a MeshEscrow address
//! is deployed.

use anyhow::{anyhow, bail, Context, Result};
use std::io::Read;
use std::path::Path;
use tkr_mesh::payment::{EscrowDomain, Receipt};
use tkr_mesh::{Address, Identity};

pub fn receipt_issue(
    session_id: &str,
    cumulative: &str,
    chain_id: u64,
    contract: &str,
    key_file: &Path,
) -> Result<()> {
    let session_bytes = parse_session_id(session_id)?;
    let cumulative: u128 = cumulative
        .parse()
        .with_context(|| format!("--cumulative must be a non-negative integer, got {cumulative:?}"))?;
    let contract: Address = contract
        .parse()
        .map_err(|e| anyhow!("--contract: {e}"))?;
    let identity = load_identity(key_file)?;

    let domain = EscrowDomain {
        chain_id,
        verifying_contract: contract,
    };
    let receipt = Receipt::issue(&identity, session_bytes, cumulative, domain);
    let json = serde_json::to_string_pretty(&receipt).context("serialize receipt")?;
    println!("{json}");

    eprintln!();
    eprintln!("payer:    {}", identity.address());
    eprintln!("session:  0x{}", hex::encode(session_bytes));
    eprintln!("cumul.:   {} (chain {chain_id}, contract {contract})", cumulative);
    Ok(())
}

pub fn receipt_verify(receipt_path: &str, expected_payer: &str) -> Result<()> {
    let raw = if receipt_path == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).context("read stdin")?;
        buf
    } else {
        std::fs::read_to_string(receipt_path)
            .with_context(|| format!("read {receipt_path}"))?
    };
    let receipt: Receipt =
        serde_json::from_str(&raw).context("parse receipt JSON")?;
    let expected: Address = expected_payer
        .parse()
        .map_err(|e| anyhow!("--payer: {e}"))?;

    let (sid, cum) = receipt.verify(expected).map_err(|e| anyhow!("verify: {e:?}"))?;
    println!("✓ receipt valid");
    println!("  payer       {expected}");
    println!("  session_id  0x{}", hex::encode(sid));
    println!("  cumulative  {cum}");
    println!("  chain_id    {}", receipt.domain.chain_id);
    println!("  contract    {}", receipt.domain.verifying_contract);
    Ok(())
}

// ---------- helpers ----------

fn parse_session_id(s: &str) -> Result<[u8; 32]> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped)
        .with_context(|| format!("--session-id hex decode: {s:?}"))?;
    if bytes.len() != 32 {
        bail!("--session-id must be 32 bytes (64 hex chars), got {}", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Load a private key from a file. Accepts either a single line of hex
/// (with or without 0x prefix) or a file in `KEY=value` shape with a
/// `TKR_PAYMENT_KEY=...` line.
fn load_identity(path: &Path) -> Result<Identity> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read key file {}", path.display()))?;
    let hex_str = pick_hex(&raw)
        .ok_or_else(|| anyhow!("no 32-byte hex private key found in {}", path.display()))?;
    let bytes = hex::decode(hex_str.strip_prefix("0x").unwrap_or(&hex_str))
        .with_context(|| format!("key file hex decode: {}", path.display()))?;
    if bytes.len() != 32 {
        bail!("key file must contain 32 bytes (64 hex chars), got {}", bytes.len());
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&bytes);
    Identity::from_secret_bytes(&secret).map_err(|e| anyhow!("identity: {e:?}"))
}

fn pick_hex(s: &str) -> Option<String> {
    // Accept either `KEY=hex` lines (for env-style files) or a bare hex line.
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let candidate = line
            .split_once('=')
            .map(|(_, v)| v.trim().trim_matches('"').trim_matches('\''))
            .unwrap_or(line);
        let stripped = candidate.strip_prefix("0x").unwrap_or(candidate);
        if stripped.len() == 64 && stripped.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(candidate.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn pick_hex_handles_env_format() {
        let env = "# comment\nTKR_PAYMENT_KEY=0x1111111111111111111111111111111111111111111111111111111111111111\n";
        let h = pick_hex(env).unwrap();
        assert!(h.starts_with("0x11"));
    }

    #[test]
    fn pick_hex_handles_bare_hex() {
        let bare = "1111111111111111111111111111111111111111111111111111111111111111\n";
        let h = pick_hex(bare).unwrap();
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn pick_hex_rejects_wrong_length() {
        assert!(pick_hex("abcd\n").is_none());
        assert!(pick_hex("0xabcd\n").is_none());
    }

    #[test]
    fn issue_then_verify_via_files() {
        // Simulate the full pipeline through the file-based API.
        let mut key_file = tempfile::NamedTempFile::new().unwrap();
        let mut secret = [0u8; 32];
        secret[31] = 1;
        let identity = Identity::from_secret_bytes(&secret).unwrap();
        let payer_addr = identity.address().to_checksum();
        writeln!(key_file, "TKR_PAYMENT_KEY=0x{}", hex::encode(secret)).unwrap();
        key_file.flush().unwrap();

        // Issue captures stdout, but for the test we call the lower-level
        // Receipt::issue directly (the CLI wrapper does the same thing).
        let domain = EscrowDomain {
            chain_id: 8453,
            verifying_contract: "0x000000000000000000000000000000000000c0de"
                .parse()
                .unwrap(),
        };
        let receipt = Receipt::issue(&identity, [0xAB; 32], 12_345, domain);
        let mut receipt_file = tempfile::NamedTempFile::new().unwrap();
        receipt_file
            .write_all(serde_json::to_string(&receipt).unwrap().as_bytes())
            .unwrap();
        receipt_file.flush().unwrap();

        // The CLI verify path:
        let res = receipt_verify(
            receipt_file.path().to_str().unwrap(),
            &payer_addr,
        );
        assert!(res.is_ok(), "verify failed: {res:?}");
    }
}
