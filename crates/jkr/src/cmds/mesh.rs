//! `jkr mesh` — peer-messaging CLI on top of jkr-mesh::Client.
//!
//! Persists JoinedMesh records to ~/.jkr/mesh/<slug>.json (chmod 0600).
//! Each record carries the secp256k1 secret used as both the mesh
//! identity and the EVM wallet for payment receipts (Phase 3).

use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use jkr_mesh::frames::Frame;
use jkr_mesh::{enroll, Address, Client, Identity, Invite, JoinedMesh};

// ---------- vault paths ----------

fn mesh_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine $HOME")?;
    let dir = home.join(".jkr").join("mesh");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

fn record_path(slug: &str) -> Result<PathBuf> {
    if slug.is_empty() || slug.contains('/') || slug.contains("..") {
        bail!("invalid slug: {slug:?}");
    }
    Ok(mesh_dir()?.join(format!("{slug}.json")))
}

fn save_record(record: &JoinedMesh) -> Result<PathBuf> {
    let path = record_path(&record.mesh_slug)?;
    record
        .save(&path)
        .map_err(|e| anyhow!("save JoinedMesh: {e:?}"))?;
    Ok(path)
}

fn load_record(slug: &str) -> Result<JoinedMesh> {
    let path = record_path(slug)?;
    JoinedMesh::load(&path)
        .map_err(|e| anyhow!("load {} (run `jkr mesh join` first): {e:?}", path.display()))
}

fn identity_of(record: &JoinedMesh) -> Result<Identity> {
    let bytes = hex::decode(record.secret_hex.strip_prefix("0x").unwrap_or(&record.secret_hex))
        .context("secret_hex decode")?;
    if bytes.len() != 32 {
        bail!("secret_hex must be 32 bytes, got {}", bytes.len());
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&bytes);
    Identity::from_secret_bytes(&secret).map_err(|e| anyhow!("identity: {e:?}"))
}

// ---------- subcommand: join ----------

pub fn join(invite_url: &str, display_name: Option<&str>) -> Result<()> {
    let invite = Invite::parse_url(invite_url)
        .map_err(|e| anyhow!("parse invite URL: {e:?}"))?;
    let token = invite
        .to_token()
        .map_err(|e| anyhow!("re-encode invite token: {e:?}"))?;
    let identity = Identity::generate();

    eprintln!("→ joining mesh {:?} ({})", invite.mesh_slug, invite.mesh_id);
    eprintln!("  broker  {}", invite.broker_url);
    eprintln!("  address {}", identity.address());

    let now = unix_secs();
    let record = enroll(&invite, &token, &identity, display_name, now)
        .map_err(|e| anyhow!("enrollment: {e:?}"))?;
    let path = save_record(&record)?;

    println!("✓ joined mesh {:?}", record.mesh_slug);
    println!("  member_id  {}", record.member_id);
    println!("  address    {}", record.address);
    println!("  saved      {}", path.display());
    println!();
    println!("Next: `jkr mesh tail {}` to receive messages.", record.mesh_slug);
    Ok(())
}

// ---------- subcommand: list ----------

pub fn list() -> Result<()> {
    let dir = mesh_dir()?;
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    if entries.is_empty() {
        println!("(no joined meshes — `jkr mesh join <invite-url>` to start)");
        return Ok(());
    }

    println!("{:<20} {:<46} {}", "slug", "address", "broker");
    println!("{}", "─".repeat(20 + 1 + 46 + 1 + 30));
    for entry in entries {
        if let Ok(rec) = JoinedMesh::load(&entry.path()) {
            println!(
                "{:<20} {:<46} {}",
                rec.mesh_slug, rec.address, rec.broker_url
            );
        }
    }
    Ok(())
}

// ---------- subcommand: whoami ----------

pub fn whoami(slug: &str) -> Result<()> {
    let record = load_record(slug)?;
    let identity = identity_of(&record)?;

    use k256::elliptic_curve::sec1::ToEncodedPoint;
    use k256::SecretKey;
    let sk = SecretKey::from_slice(&identity.secret_bytes())
        .map_err(|e| anyhow!("secret: {e}"))?;
    let pub_compressed = sk.public_key().to_encoded_point(true);

    println!("mesh    {}", record.mesh_slug);
    println!("address {}", identity.address());
    println!("pubkey  0x{}", hex::encode(pub_compressed.as_bytes()));
    println!();
    println!("Share the pubkey with peers who want to send you DMs.");
    Ok(())
}

// ---------- subcommand: tail ----------

pub fn tail(slug: &str, reconnect: bool) -> Result<()> {
    let record = load_record(slug)?;
    let identity = identity_of(&record)?;
    let my_addr = identity.address();

    eprintln!("→ tailing mesh {:?}", slug);
    eprintln!("  address {my_addr}");
    eprintln!("  broker  {}", record.broker_url);
    if reconnect {
        eprintln!("  reconnect on (1s→60s backoff with jitter)");
    }
    eprintln!("  press ctrl-c to exit");
    eprintln!();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;

    rt.block_on(async move {
        // Backoff state lives across reconnect attempts. A successful drain
        // resets it so a long-lived peer that drops once doesn't carry a
        // 60s penalty into its next blip.
        let mut backoff_secs: u64 = 1;
        loop {
            match Client::connect(&record, identity.clone()).await {
                Ok(mut client) => {
                    eprintln!("✓ connected\n");
                    backoff_secs = 1;
                    loop {
                        match client.next().await {
                            None => {
                                eprintln!("(connection closed)");
                                break;
                            }
                            Some(Frame::Push(p)) => print_push(&p, &identity),
                            Some(Frame::Error(e)) => {
                                eprintln!("[broker error] {} ({})", e.message, e.code);
                            }
                            // Ack/Hello/Send shouldn't arrive at the client; ignore.
                            Some(_) => {}
                        }
                    }
                }
                Err(e) => {
                    if !reconnect {
                        return Err(anyhow!("connect: {e:?}"));
                    }
                    eprintln!("[connect failed] {e:?}");
                }
            }

            if !reconnect {
                return Ok(());
            }

            // Jitter ±25% to avoid thundering-herd reconnect when N peers all
            // drop together (e.g. broker restart). rand 0..1 mapped to 0.75..1.25.
            let jitter: f64 = 0.75 + rand::random::<f64>() * 0.5;
            let sleep_ms = ((backoff_secs as f64) * 1000.0 * jitter) as u64;
            eprintln!("  reconnecting in {:.1}s…", sleep_ms as f64 / 1000.0);
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
            backoff_secs = (backoff_secs * 2).min(60);
        }
    })
}

fn print_push(p: &jkr_mesh::frames::PushFields, identity: &Identity) {
    let ts = chrono::Utc::now().format("%H:%M:%S");
    match p.envelope.open(identity) {
        Ok(plaintext) => match std::str::from_utf8(&plaintext) {
            Ok(s) => println!("[{ts}] {} → {}", p.from, s),
            Err(_) => println!(
                "[{ts}] {} → ({} bytes binary)",
                p.from,
                plaintext.len()
            ),
        },
        Err(e) => eprintln!("[{ts}] {} → (decrypt failed: {e:?})", p.from),
    }
}

// ---------- subcommand: send ----------

pub fn send(slug: &str, to: &str, recipient_pubkey: &str, message: &str) -> Result<()> {
    let record = load_record(slug)?;
    let identity = identity_of(&record)?;

    let to_addr: Address = to.parse().map_err(|e| anyhow!("--to: {e:?}"))?;
    let pub_bytes = hex::decode(recipient_pubkey.strip_prefix("0x").unwrap_or(recipient_pubkey))
        .context("--recipient-pubkey hex")?;
    let recipient_pub = k256::PublicKey::from_sec1_bytes(&pub_bytes)
        .map_err(|e| anyhow!("--recipient-pubkey parse: {e}"))?;

    let body = if message == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).context("read stdin")?;
        buf
    } else {
        message.to_string()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;

    rt.block_on(async move {
        let client = Client::connect(&record, identity)
            .await
            .map_err(|e| anyhow!("connect: {e:?}"))?;
        let id = client
            .send_dm(to_addr, &recipient_pub, body.as_bytes())
            .await
            .map_err(|e| anyhow!("send_dm: {e:?}"))?;
        eprintln!("✓ queued");
        eprintln!("  msg id  {id}");
        eprintln!("  to      {to_addr}");
        eprintln!("  bytes   {}", body.len());
        // Give the writer task a moment to flush.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

// ---------- subcommand: invite-mint ----------

pub fn invite_mint(
    slug: &str,
    broker_url: &str,
    owner_key_file: &std::path::Path,
    ttl_hours: u64,
) -> Result<()> {
    let owner = load_identity_from_file(owner_key_file)?;
    let mesh_id = format!("mesh_{slug}_{}", &owner.address().to_string()[2..10]);
    let expires_at = unix_secs() + ttl_hours.saturating_mul(3600);

    let invite = Invite::issue(
        &owner,
        mesh_id.clone(),
        slug.to_string(),
        broker_url.to_string(),
        expires_at,
        jkr_mesh::Role::Member,
    );
    let url = invite
        .to_url(broker_url)
        .map_err(|e| anyhow!("to_url: {e:?}"))?;

    eprintln!("→ minted invite for mesh {slug:?}");
    eprintln!("  owner       {}", owner.address());
    eprintln!("  mesh_id     {mesh_id}");
    eprintln!("  expires     {} ({}h from now)", expires_at, ttl_hours);
    eprintln!("  broker      {broker_url}");
    eprintln!();
    println!("{url}");
    Ok(())
}

fn load_identity_from_file(path: &std::path::Path) -> Result<Identity> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read key file {}", path.display()))?;
    // Accept either `KEY=hex` lines or a bare hex line (same shape as
    // the loader in cmds/pay.rs). We re-implement here to avoid a
    // cross-module dep.
    for line in raw.lines() {
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
            let bytes = hex::decode(stripped).context("hex decode")?;
            let mut secret = [0u8; 32];
            secret.copy_from_slice(&bytes);
            return Identity::from_secret_bytes(&secret).map_err(|e| anyhow!("identity: {e:?}"));
        }
    }
    bail!("no 32-byte hex private key found in {}", path.display())
}

// ---------- helpers ----------

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_path_rejects_traversal() {
        assert!(record_path("").is_err());
        assert!(record_path("..").is_err());
        assert!(record_path("a/b").is_err());
        assert!(record_path("ok-slug").is_ok());
    }
}
