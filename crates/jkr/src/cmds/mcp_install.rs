//! `tkr mcp install` — wire up the tkr MCP server in Claude Code's config.
//!
//! Project scope writes `./.mcp.json` (per-repo, lives in source control).
//! User scope updates `~/.claude.json` so tkr is available in every project.
//! Idempotent: re-running updates the `tkr` entry in place without touching
//! other MCP servers in the same file.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use crate::cli::McpScope;

/// The canonical `tkr` server entry. Kept here so the format is one place
/// to update if the args ever change.
fn tkr_server_entry() -> Value {
    json!({
        "command": "tkr",
        "args": ["mcp"],
    })
}

fn project_config_path() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("getcwd")?;
    Ok(cwd.join(".mcp.json"))
}

fn user_config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".claude.json"))
}

pub fn run(scope: McpScope, print: bool, force: bool) -> Result<()> {
    let entry = tkr_server_entry();
    let entry_str = serde_json::to_string_pretty(&json!({
        "mcpServers": { "tkr": entry.clone() }
    }))?;

    if print {
        println!("{entry_str}");
        return Ok(());
    }

    match scope {
        McpScope::Project => install_project(entry, force),
        McpScope::User => install_user(entry, force),
    }
}

/// Write `./.mcp.json`. If the file doesn't exist, create it with just the
/// tkr server. If it exists, merge the tkr entry in (and preserve any other
/// servers already there).
fn install_project(entry: Value, force: bool) -> Result<()> {
    let path = project_config_path()?;
    let mut doc: Value = if path.exists() {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("parse {} as JSON", path.display()))?
    } else {
        json!({ "mcpServers": {} })
    };

    let servers = doc
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| anyhow!("{}: `mcpServers` must be an object", path.display()))?;

    if servers.contains_key("tkr") && !force {
        println!(
            "tkr mcp install: `tkr` already configured at {} (pass --force to overwrite)",
            path.display()
        );
        return Ok(());
    }
    servers.insert("tkr".into(), entry);

    let serialized = serde_json::to_string_pretty(&doc)? + "\n";
    fs::write(&path, serialized).with_context(|| format!("write {}", path.display()))?;
    println!(
        "tkr mcp install: wrote `tkr` server entry to {}",
        path.display()
    );
    println!("  Restart Claude Code in this repo to pick it up.");
    Ok(())
}

/// Update `~/.claude.json`. Claude Code stores user-level MCP servers under
/// the top-level `mcpServers` key in that file. Some setups instead use
/// per-project `projects.<cwd>.mcpServers`; we don't touch that — if the
/// agent wants per-project scoping, they should use --scope=project (which
/// writes ./.mcp.json) instead.
fn install_user(entry: Value, force: bool) -> Result<()> {
    let path = user_config_path()?;
    if !path.exists() {
        bail!(
            "{} does not exist — is Claude Code installed for this user?",
            path.display()
        );
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let mut doc: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse {} as JSON", path.display()))?;

    let obj = doc
        .as_object_mut()
        .ok_or_else(|| anyhow!("{}: top-level must be a JSON object", path.display()))?;

    // Insert mcpServers if absent (Claude Code's user config doesn't always
    // ship with the key; first-time MCP users start with no top-level entry).
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("{}: `mcpServers` must be an object", path.display()))?;

    if servers.contains_key("tkr") && !force {
        println!(
            "tkr mcp install: `tkr` already configured at {} (pass --force to overwrite)",
            path.display()
        );
        return Ok(());
    }
    servers.insert("tkr".into(), entry);

    let serialized = serde_json::to_string_pretty(&doc)? + "\n";
    fs::write(&path, serialized).with_context(|| format!("write {}", path.display()))?;
    println!(
        "tkr mcp install: wrote `tkr` server entry to {}",
        path.display()
    );
    println!("  Restart Claude Code to pick it up. Now active in every project.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Snapshot-style test: create a project-scope install in a temp dir and
    /// verify the resulting file parses + contains the tkr entry.
    #[test]
    fn project_install_writes_mcp_json() {
        let dir = tempdir().unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let r = install_project(tkr_server_entry(), false);
        std::env::set_current_dir(prev).unwrap();
        r.unwrap();
        let raw = fs::read_to_string(dir.path().join(".mcp.json")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(doc["mcpServers"]["tkr"]["command"], "tkr");
        assert_eq!(doc["mcpServers"]["tkr"]["args"][0], "mcp");
    }

    /// Re-running on a config that already has `tkr` should no-op unless
    /// --force. Other servers must be preserved.
    #[test]
    fn project_install_preserves_existing_servers_and_noops_without_force() {
        let dir = tempdir().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        let initial = json!({
            "mcpServers": {
                "other": { "command": "other", "args": [] },
                "tkr": { "command": "tkr-old", "args": ["mcp", "--legacy"] }
            }
        });
        fs::write(&mcp_path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        install_project(tkr_server_entry(), false).unwrap();
        std::env::set_current_dir(prev).unwrap();

        let doc: Value =
            serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
        // Existing `other` server preserved.
        assert_eq!(doc["mcpServers"]["other"]["command"], "other");
        // `tkr` entry NOT overwritten without --force.
        assert_eq!(doc["mcpServers"]["tkr"]["command"], "tkr-old");
    }

    #[test]
    fn project_install_overwrites_tkr_with_force() {
        let dir = tempdir().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        fs::write(
            &mcp_path,
            serde_json::to_string_pretty(&json!({
                "mcpServers": { "tkr": { "command": "tkr-old", "args": [] } }
            }))
            .unwrap(),
        )
        .unwrap();

        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        install_project(tkr_server_entry(), true).unwrap();
        std::env::set_current_dir(prev).unwrap();

        let doc: Value =
            serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
        assert_eq!(doc["mcpServers"]["tkr"]["command"], "tkr");
        assert_eq!(doc["mcpServers"]["tkr"]["args"][0], "mcp");
    }
}
