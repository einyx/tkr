//! `tkr install` — wire the tkr hook into AI coding tools.
//!
//! Supports Claude Code (~/.claude/settings.json) and Codex CLI
//! (~/.codex/config.toml). Auto-detects installed tools when no flag is given.

use anyhow::{Context, Result};
use serde_json::{json, Value};

pub fn run(only_claude: bool, only_codex: bool) -> Result<()> {
    let home = dirs::home_dir().context("could not determine $HOME")?;

    let bin = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tkr".into());

    // When no flag is given, auto-detect installed tools.
    let auto = !only_claude && !only_codex;

    let do_claude = only_claude || (auto && home.join(".claude").exists());
    let do_codex = only_codex || (auto && home.join(".codex").exists());

    if !do_claude && !do_codex {
        println!("No supported AI tools detected. Supported: Claude Code, Codex CLI.");
        println!("Run with --claude or --codex to install anyway.");
        return Ok(());
    }

    if do_claude {
        install_claude(&home, &bin)?;
    }
    if do_codex {
        install_codex(&home, &bin)?;
    }

    Ok(())
}

// ── Claude Code ───────────────────────────────────────────────────────────────

fn install_claude(home: &std::path::Path, bin: &str) -> Result<()> {
    let settings_path = home.join(".claude").join("settings.json");
    let hook_command = format!("{bin} hook claude");

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

    let root = settings.as_object_mut().context("settings.json must be a JSON object")?;
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

    let bash_idx = pretool.iter().position(|m| {
        m.get("matcher").and_then(|x| x.as_str()).map(|s| s == "Bash").unwrap_or(false)
    });
    let bash_entry = match bash_idx {
        Some(i) => &mut pretool[i],
        None => {
            pretool.push(json!({ "matcher": "Bash", "hooks": [] }));
            pretool.last_mut().unwrap()
        }
    };
    let bash_hooks = bash_entry
        .as_object_mut()
        .context("Bash matcher must be an object")?
        .entry("hooks")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("Bash.hooks must be an array")?;

    let already = bash_hooks.iter().any(|h| {
        h.get("command").and_then(|c| c.as_str())
            .map(|s| s.contains("tkr") && s.contains("hook claude"))
            .unwrap_or(false)
    });

    if already {
        println!("✓ Claude Code: already installed at {}", settings_path.display());
        return Ok(());
    }

    bash_hooks.push(json!({ "type": "command", "command": hook_command }));
    let serialized = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, serialized + "\n")
        .with_context(|| format!("writing {}", settings_path.display()))?;

    println!(
        "✓ Claude Code: installed into {}\n  Restart Claude Code to activate.",
        settings_path.display()
    );
    Ok(())
}

// ── Codex CLI ─────────────────────────────────────────────────────────────────
//
// Codex uses the same PreToolUse JSON protocol as Claude Code, but its hooks
// live in ~/.codex/config.toml under [hooks.PreToolUse].
//
// Target format:
//   [[hooks.PreToolUse]]
//   matcher = "LocalShell"
//   [[hooks.PreToolUse.hooks]]
//   type = "command"
//   command = "/path/to/tkr hook claude"

fn install_codex(home: &std::path::Path, bin: &str) -> Result<()> {
    let config_path = home.join(".codex").join("config.toml");
    let hook_command = format!("{bin} hook claude");

    let existing = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .with_context(|| format!("reading {}", config_path.display()))?
    } else {
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        String::new()
    };

    if existing.contains("tkr") && existing.contains("hook claude") {
        println!("✓ Codex: already installed at {}", config_path.display());
        return Ok(());
    }

    let hook_block = format!(
        "\n[[hooks.PreToolUse]]\nmatcher = \"LocalShell\"\n\n  [[hooks.PreToolUse.hooks]]\n  type = \"command\"\n  command = \"{hook_command}\"\n"
    );

    let new_content = existing + &hook_block;
    std::fs::write(&config_path, new_content)
        .with_context(|| format!("writing {}", config_path.display()))?;

    println!(
        "✓ Codex: installed into {}\n  Restart Codex to activate.",
        config_path.display()
    );
    Ok(())
}
