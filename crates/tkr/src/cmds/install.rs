//! `tkr install` — wire the tkr Bash hook into `~/.claude/settings.json`.
//!
//! Adds a PreToolUse / Bash matcher entry whose `command` is the absolute
//! path to the running `tkr` binary plus `hook claude`. Idempotent — running
//! it twice does not duplicate the entry. Existing unrelated hooks (other
//! tools, other matchers) are preserved unchanged.

use anyhow::{Context, Result};
use serde_json::{json, Value};

pub fn run() -> Result<()> {
    let home = dirs::home_dir().context("could not determine $HOME")?;
    let settings_path = home.join(".claude").join("settings.json");

    let mut settings: Value = if settings_path.exists() {
        let text = std::fs::read_to_string(&settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?;
        if text.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(&text)
                .with_context(|| format!("parsing {}", settings_path.display()))?
        }
    } else {
        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        json!({})
    };

    // Resolve the path to the running tkr binary so the hook works regardless
    // of PATH inside Claude Code's hook subshell.
    let bin = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tkr".into());
    let hook_command = format!("{} hook claude", bin);

    // Drill into hooks.PreToolUse[] and ensure a Bash matcher with our entry exists.
    let root = settings
        .as_object_mut()
        .context("settings.json must be a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("hooks must be a JSON object")?;
    let pretool = hooks
        .entry("PreToolUse")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("hooks.PreToolUse must be an array")?;

    // Find an existing Bash matcher entry, else create one.
    let bash_idx = pretool.iter().position(|m| {
        m.get("matcher")
            .and_then(|x| x.as_str())
            .map(|s| s == "Bash")
            .unwrap_or(false)
    });

    let bash_entry = match bash_idx {
        Some(i) => &mut pretool[i],
        None => {
            pretool.push(json!({ "matcher": "Bash", "hooks": [] }));
            pretool.last_mut().unwrap()
        }
    };

    let bash_obj = bash_entry
        .as_object_mut()
        .context("Bash matcher must be an object")?;
    let bash_hooks = bash_obj
        .entry("hooks")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("Bash.hooks must be an array")?;

    let already = bash_hooks.iter().any(|h| {
        h.get("command")
            .and_then(|c| c.as_str())
            .map(|s| s.contains("tkr") && s.contains("hook claude"))
            .unwrap_or(false)
    });

    if already {
        println!("✓ tkr hook is already installed at {}", settings_path.display());
        return Ok(());
    }

    bash_hooks.push(json!({ "type": "command", "command": hook_command.clone() }));

    let serialized = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, serialized + "\n")
        .with_context(|| format!("writing {}", settings_path.display()))?;

    println!(
        "✓ Installed tkr hook into {}\n  Command: {}\n  Restart Claude Code (or start a new session) to activate.",
        settings_path.display(),
        hook_command,
    );
    Ok(())
}
