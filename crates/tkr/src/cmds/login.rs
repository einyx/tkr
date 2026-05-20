//! `tkr login` / `tkr whoami` / `tkr logout`.
//!
//! Replaces the `TKR_INGEST_TOKEN` env var workflow with a real
//! per-user bearer minted by tkr-server behind a Logto session. The
//! token is stored in the OS keychain (Secret Service on Linux,
//! Keychain on macOS) and read by `emit_ingest()` in `cmds::sandbox`
//! whenever the CLI reports a sandbox run.
//!
//! Flow:
//!   1. User runs `tkr login --url https://tkr.prysm.sh`.
//!   2. CLI opens the dashboard URL in the browser. The user signs
//!      in to Logto if they aren't already, clicks "Generate CLI
//!      token" in the CLI Tokens panel, and copies the resulting
//!      token.
//!   3. CLI prompts for the pasted token on stdin and stores it in
//!      the keychain under (service="tkr-cli", user=`<url>`).
//!
//! Alternatively users can pass `--token <value>` directly (useful
//! for scripted setup) or pre-mint via the dashboard.

use anyhow::{anyhow, bail, Context, Result};
use std::io::{BufRead, Write};

const KEYRING_SERVICE: &str = "tkr-cli";
const URL_KEY: &str = "tkr-cli/url";

/// Open the dashboard (or print the URL) and read a pasted token
/// from stdin, then persist (url, token) in the keychain.
pub fn login(url: &str, token: Option<&str>, no_browser: bool) -> Result<()> {
    let url = url.trim_end_matches('/').to_string();
    if url.is_empty() {
        bail!("--url is required");
    }

    let token = match token {
        Some(t) => t.trim().to_string(),
        None => {
            let dash_url = format!("{url}/dashboard");
            if no_browser {
                println!("open this in a browser to mint a CLI token:");
                println!("  {dash_url}");
            } else {
                println!("opening {dash_url} — sign in and generate a CLI token …");
                let _ = open_browser(&dash_url);
            }
            print!("paste token: ");
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .context("read token from stdin")?;
            line.trim().to_string()
        }
    };
    if token.is_empty() {
        bail!("no token provided");
    }
    if !token.starts_with("tkr_") {
        eprintln!(
            "warning: token does not start with `tkr_` — \
             continuing anyway in case the format changes"
        );
    }

    keyring_set(URL_KEY, &url).context("store URL in keychain")?;
    keyring_set(&token_key(&url), &token).context("store token in keychain")?;
    println!("✓ stored token for {url}");
    println!("  test with: tkr whoami");
    Ok(())
}

pub fn whoami() -> Result<()> {
    let url = match keyring_get(URL_KEY) {
        Ok(u) => u,
        Err(_) => {
            println!("not signed in. run `tkr login`.");
            return Ok(());
        }
    };
    let token = keyring_get(&token_key(&url)).ok();
    println!("server:  {url}");
    match token {
        Some(t) => println!("token:   {}… (stored in keychain)", &t[..t.len().min(12)]),
        None => println!("token:   (none — run `tkr login` again)"),
    }
    Ok(())
}

pub fn logout() -> Result<()> {
    let url = keyring_get(URL_KEY).unwrap_or_default();
    if !url.is_empty() {
        let _ = keyring_delete(&token_key(&url));
    }
    let _ = keyring_delete(URL_KEY);
    println!("✓ tkr credentials cleared");
    Ok(())
}

/// Look up `(url, token)` from the keychain so other CLI commands
/// (notably the sandbox-ingest emitter) can authenticate without
/// reading env vars. Returns None if either is missing.
pub fn stored_credentials() -> Option<(String, String)> {
    let url = keyring_get(URL_KEY).ok()?;
    let token = keyring_get(&token_key(&url)).ok()?;
    Some((url, token))
}

fn token_key(url: &str) -> String {
    format!("tkr-cli/token:{url}")
}

// On laptops the OS keychain is the right home for the token. On
// headless servers + CI there's no Secret Service / Keychain daemon,
// so we fall back to a 0600-perm file under ~/.config/tkr/. That's
// equivalent security-wise to env-var storage but persists across
// shells. Both backends are tried, keychain first.
fn keyring_set(key: &str, value: &str) -> Result<()> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, key) {
        if entry.set_password(value).is_ok() {
            return Ok(());
        }
    }
    file_set(key, value)
}

fn keyring_get(key: &str) -> Result<String> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, key) {
        if let Ok(v) = entry.get_password() {
            return Ok(v);
        }
    }
    file_get(key)
}

fn keyring_delete(key: &str) -> Result<()> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, key) {
        let _ = entry.delete_credential();
    }
    let _ = file_delete(key);
    Ok(())
}

fn credentials_path() -> Result<std::path::PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| anyhow!("could not resolve XDG config dir"))?
        .join("tkr");
    std::fs::create_dir_all(&dir).context("create config dir")?;
    Ok(dir.join("credentials.toml"))
}

fn file_load() -> Result<toml::Table> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(toml::Table::new());
    }
    let raw = std::fs::read_to_string(&path).context("read credentials file")?;
    toml::from_str::<toml::Table>(&raw).context("parse credentials file")
}

fn file_save(table: &toml::Table) -> Result<()> {
    let path = credentials_path()?;
    let body = toml::to_string(table).context("encode credentials file")?;
    std::fs::write(&path, body).context("write credentials file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn file_set(key: &str, value: &str) -> Result<()> {
    let mut t = file_load().unwrap_or_default();
    t.insert(key.to_string(), toml::Value::String(value.to_string()));
    file_save(&t)
}

fn file_get(key: &str) -> Result<String> {
    let t = file_load()?;
    t.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("key `{key}` not found"))
}

fn file_delete(key: &str) -> Result<()> {
    let mut t = file_load().unwrap_or_default();
    t.remove(key);
    file_save(&t)
}

fn open_browser(url: &str) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| anyhow!("could not launch browser ({opener}): {e}"))
}
