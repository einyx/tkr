//! `jkr job` — agent JobBoard CLI. Post tasks, take open ones, deliver
//! results, accept work, cancel/timeout. All on-chain via alloy; the
//! actual job spec + result delivery happens off-chain (mesh DMs).

use anyhow::{anyhow, bail, Context, Result};
use std::path::Path;

use alloy::network::EthereumWallet;
use alloy::primitives::{Address as EvmAddr, FixedBytes, U256};
use alloy::providers::ProviderBuilder;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;

sol! {
    #[sol(rpc)]
    interface IJobBoard {
        function postJob(bytes32 specHash, string calldata specPreview, uint256 reward, address token, uint64 deadline) external payable returns (uint256);
        function takeJob(uint256 id) external;
        function completeJob(uint256 id, bytes32 resultHash) external;
        function acceptCompletion(uint256 id) external;
        function cancelJob(uint256 id) external;
        function timeoutClaim(uint256 id) external;
        function jobCount() external view returns (uint256);
        function getJob(uint256 id) external view returns (
            address poster,
            address worker,
            uint256 reward,
            address token,
            bytes32 specHash,
            bytes32 resultHash,
            uint64 deadline,
            uint8 status,
            string memory specPreview
        );
    }
}

// ---------- arg parsing helpers ----------

fn parse_addr(s: &str) -> Result<EvmAddr> {
    s.parse::<EvmAddr>().map_err(|e| anyhow!("address parse {s:?}: {e}"))
}

fn parse_bytes32(s: &str) -> Result<FixedBytes<32>> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    let bytes = hex::decode(stripped).context("hex decode")?;
    if bytes.len() != 32 {
        bail!("expected 32 bytes, got {}", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(FixedBytes::from(out))
}

fn parse_u256(s: &str) -> Result<U256> {
    if let Some(rest) = s.strip_prefix("0x") {
        U256::from_str_radix(rest, 16).map_err(|e| anyhow!("u256 hex {s:?}: {e}"))
    } else {
        U256::from_str_radix(s, 10).map_err(|e| anyhow!("u256 dec {s:?}: {e}"))
    }
}

fn pick_hex_key(s: &str) -> Option<String> {
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

fn load_signer(path: &Path) -> Result<PrivateKeySigner> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let hex_str = pick_hex_key(&raw)
        .ok_or_else(|| anyhow!("no 32-byte hex private key found in {}", path.display()))?;
    let bytes = hex::decode(hex_str.strip_prefix("0x").unwrap_or(&hex_str)).context("key file hex decode")?;
    if bytes.len() != 32 {
        bail!("key file must contain 32 bytes (64 hex chars), got {}", bytes.len());
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&bytes);
    PrivateKeySigner::from_bytes(&FixedBytes::from(secret))
        .map_err(|e| anyhow!("alloy signer: {e}"))
}

fn rt() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("tokio runtime")
}

// ---------- Subcommands ----------

#[allow(clippy::too_many_arguments)]
pub fn post(
    preview: &str,
    spec_hash: &str,
    reward: &str,
    token: &str,
    deadline: u64,
    board: &str,
    rpc_url: &str,
    key_file: &Path,
) -> Result<()> {
    let board_addr: EvmAddr = parse_addr(board)?;
    let token_addr: EvmAddr = parse_addr(token)?;
    let spec_hash = parse_bytes32(spec_hash)?;
    let reward_u256 = parse_u256(reward)?;
    if preview.len() > 256 {
        bail!("--preview must be ≤256 chars, got {}", preview.len());
    }
    let signer = load_signer(key_file)?;
    let from = signer.address();
    let wallet = EthereumWallet::from(signer);

    let rt = rt()?;
    rt.block_on(async {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(rpc_url.parse().context("rpc_url parse")?);
        let board_c = IJobBoard::new(board_addr, &provider);

        eprintln!("→ postJob");
        eprintln!("  poster   {from}");
        eprintln!("  reward   {reward_u256}");
        eprintln!("  token    {token_addr}");
        eprintln!("  deadline {deadline}");

        let mut call = board_c.postJob(spec_hash, preview.to_string(), reward_u256, token_addr, deadline);
        if token_addr == EvmAddr::ZERO {
            call = call.value(reward_u256);
        }
        let pending = call.send().await.map_err(|e| anyhow!("postJob send: {e}"))?;
        let tx = *pending.tx_hash();
        eprintln!("  tx hash  {tx:#x}");
        let receipt = pending.get_receipt().await.map_err(|e| anyhow!("await receipt: {e}"))?;
        if !receipt.status() {
            bail!("postJob reverted on-chain");
        }
        let count = board_c.jobCount().call().await.map_err(|e| anyhow!("jobCount: {e}"))?;
        let id: U256 = count._0;
        println!("✓ posted as job #{id}");
        println!("  block    {}", receipt.block_number.unwrap_or_default());
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}

pub fn list(board: &str, rpc_url: &str, limit: usize) -> Result<()> {
    let board_addr: EvmAddr = parse_addr(board)?;
    let rt = rt()?;
    rt.block_on(async {
        let provider = ProviderBuilder::new().on_http(rpc_url.parse().context("rpc_url parse")?);
        let board_c = IJobBoard::new(board_addr, &provider);

        let count = board_c.jobCount().call().await.map_err(|e| anyhow!("jobCount: {e}"))?;
        let count_u: u64 = count._0.try_into().unwrap_or(0);
        println!("{} job(s) total on board {}", count_u, board_addr);
        if count_u == 0 {
            return Ok::<_, anyhow::Error>(());
        }

        let take_n = count_u.min(limit as u64);
        println!();
        println!("{:>4}  {:<10}  {:<24}  {:<10}  {}", "id", "status", "reward (wei)", "deadline", "preview");
        println!("{}", "─".repeat(80));
        for id in 1..=take_n {
            let r = board_c.getJob(U256::from(id)).call().await
                .map_err(|e| anyhow!("getJob {id}: {e}"))?;
            let status = match r.status {
                0 => "Open",
                1 => "Taken",
                2 => "Completed",
                3 => "Accepted",
                4 => "Cancelled",
                5 => "TimedOut",
                _ => "?",
            };
            let preview = if r.specPreview.len() > 50 {
                format!("{}…", &r.specPreview[..50])
            } else {
                r.specPreview
            };
            println!("{:>4}  {:<10}  {:<24}  {:<10}  {}", id, status, r.reward, r.deadline, preview);
        }
        Ok(())
    })?;
    Ok(())
}

// Each by-id mutator below repeats the small connect/send/await pattern.
// Trying to factor it out via a higher-order helper drags in alloy's full
// FillProvider type signature; inlining is cleaner.

pub fn take(id: u64, board: &str, rpc_url: &str, key_file: &Path) -> Result<()> {
    let board_addr: EvmAddr = parse_addr(board)?;
    let signer = load_signer(key_file)?;
    let from = signer.address();
    let wallet = EthereumWallet::from(signer);
    let rt = rt()?;
    rt.block_on(async {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(rpc_url.parse().context("rpc_url parse")?);
        let board_c = IJobBoard::new(board_addr, &provider);
        eprintln!("→ takeJob({id}) by {from}");
        let pending = board_c.takeJob(U256::from(id)).send().await
            .map_err(|e| anyhow!("takeJob send: {e}"))?;
        eprintln!("  tx hash {:#x}", *pending.tx_hash());
        let receipt = pending.get_receipt().await.map_err(|e| anyhow!("await receipt: {e}"))?;
        if !receipt.status() { bail!("takeJob reverted"); }
        println!("✓ took job #{id}");
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}

pub fn complete(id: u64, result_hash: &str, board: &str, rpc_url: &str, key_file: &Path) -> Result<()> {
    let board_addr: EvmAddr = parse_addr(board)?;
    let result_hash = parse_bytes32(result_hash)?;
    let signer = load_signer(key_file)?;
    let from = signer.address();
    let wallet = EthereumWallet::from(signer);
    let rt = rt()?;
    rt.block_on(async {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(rpc_url.parse().context("rpc_url parse")?);
        let board_c = IJobBoard::new(board_addr, &provider);
        eprintln!("→ completeJob({id}) by {from}");
        let pending = board_c.completeJob(U256::from(id), result_hash).send().await
            .map_err(|e| anyhow!("completeJob send: {e}"))?;
        eprintln!("  tx hash {:#x}", *pending.tx_hash());
        let receipt = pending.get_receipt().await.map_err(|e| anyhow!("await receipt: {e}"))?;
        if !receipt.status() { bail!("completeJob reverted"); }
        println!("✓ completed job #{id}");
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}

pub fn accept(id: u64, board: &str, rpc_url: &str, key_file: &Path) -> Result<()> {
    let board_addr: EvmAddr = parse_addr(board)?;
    let signer = load_signer(key_file)?;
    let from = signer.address();
    let wallet = EthereumWallet::from(signer);
    let rt = rt()?;
    rt.block_on(async {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(rpc_url.parse().context("rpc_url parse")?);
        let board_c = IJobBoard::new(board_addr, &provider);
        eprintln!("→ acceptCompletion({id}) by {from}");
        let pending = board_c.acceptCompletion(U256::from(id)).send().await
            .map_err(|e| anyhow!("acceptCompletion send: {e}"))?;
        eprintln!("  tx hash {:#x}", *pending.tx_hash());
        let receipt = pending.get_receipt().await.map_err(|e| anyhow!("await receipt: {e}"))?;
        if !receipt.status() { bail!("acceptCompletion reverted"); }
        println!("✓ accepted job #{id} — payment released to worker");
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}

pub fn cancel(id: u64, board: &str, rpc_url: &str, key_file: &Path) -> Result<()> {
    let board_addr: EvmAddr = parse_addr(board)?;
    let signer = load_signer(key_file)?;
    let from = signer.address();
    let wallet = EthereumWallet::from(signer);
    let rt = rt()?;
    rt.block_on(async {
        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet)
            .on_http(rpc_url.parse().context("rpc_url parse")?);
        let board_c = IJobBoard::new(board_addr, &provider);
        eprintln!("→ cancelJob({id}) by {from}");
        let pending = board_c.cancelJob(U256::from(id)).send().await
            .map_err(|e| anyhow!("cancelJob send: {e}"))?;
        eprintln!("  tx hash {:#x}", *pending.tx_hash());
        let receipt = pending.get_receipt().await.map_err(|e| anyhow!("await receipt: {e}"))?;
        if !receipt.status() { bail!("cancelJob reverted"); }
        println!("✓ cancelled job #{id} — escrow refunded");
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}
