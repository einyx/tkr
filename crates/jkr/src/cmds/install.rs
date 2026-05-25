//! `tkr install` — wire the tkr hook into AI coding tools.
//!
//! Supports Claude Code (~/.claude/settings.json) and Codex CLI
//! (~/.codex/config.toml). Auto-detects installed tools when no flag is given.

use anyhow::{Context, Result};
use serde_json::{json, Value};

pub fn run(
    only_claude: bool,
    only_codex: bool,
    only_cursor: bool,
    with_foundry: bool,
) -> Result<()> {
    let home = dirs::home_dir().context("could not determine $HOME")?;

    let bin = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tkr".into());

    // When no AI-tool flag is given, auto-detect installed tools.
    let auto = !only_claude && !only_codex && !only_cursor;

    let do_claude = only_claude || (auto && home.join(".claude").exists());
    let do_codex  = only_codex  || (auto && home.join(".codex").exists());
    let do_cursor = only_cursor || (auto && home.join(".cursor").exists());

    if !do_claude && !do_codex && !do_cursor && !with_foundry {
        println!("No supported AI tools detected. Supported: Claude Code, Codex CLI, Cursor.");
        println!("Run with --claude, --codex, or --cursor to install anyway.");
        println!("Run with --with-foundry to install the smart-contract toolchain.");
        return Ok(());
    }

    if do_claude {
        install_claude(&home, &bin)?;
    }
    if do_codex {
        install_codex(&home, &bin)?;
    }
    if do_cursor {
        install_cursor(&home)?;
    }
    if with_foundry {
        install_foundry(&home)?;
    }

    Ok(())
}

/// Install foundry (forge/anvil/cast) via the official foundryup script.
/// Skips if `forge` is already discoverable on PATH.
fn install_foundry(home: &std::path::Path) -> Result<()> {
    if which("forge").is_some() {
        let version = std::process::Command::new("forge")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.lines().next().unwrap_or("forge").to_string())
            .unwrap_or_else(|| "forge".to_string());
        println!("✓ foundry: already installed ({})", version.trim());
        return Ok(());
    }

    let os = std::env::consts::OS;
    if os == "windows" {
        anyhow::bail!(
            "automatic foundry install not supported on Windows. \
             See https://book.getfoundry.sh/getting-started/installation"
        );
    }

    println!("→ foundry: installing via official foundryup script");
    println!("  source: https://foundry.paradigm.xyz");
    println!("  target: {}/.foundry/bin", home.display());
    println!();

    // Stage 1: download + run the foundryup installer (writes to ~/.foundry/bin/foundryup).
    let installer = std::process::Command::new("sh")
        .arg("-c")
        .arg("curl -L --silent --show-error https://foundry.paradigm.xyz | bash")
        .status()
        .context("invoking foundry installer")?;
    if !installer.success() {
        anyhow::bail!("foundry installer exited non-zero — check network / shell rc");
    }

    // Stage 2: run foundryup to fetch forge/anvil/cast/chisel binaries.
    let foundryup = home.join(".foundry/bin/foundryup");
    if !foundryup.exists() {
        anyhow::bail!(
            "foundryup not found at {} after installer ran",
            foundryup.display()
        );
    }
    let status = std::process::Command::new(&foundryup)
        .status()
        .with_context(|| format!("running {}", foundryup.display()))?;
    if !status.success() {
        anyhow::bail!("foundryup exited non-zero");
    }

    // Verify by trying to run forge from the expected location (PATH may not
    // include ~/.foundry/bin in this process — installer rewrites shell rc
    // for future sessions).
    let forge = home.join(".foundry/bin/forge");
    let version = std::process::Command::new(&forge)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_default();

    println!();
    println!("✓ foundry installed: {version}");
    println!("  binaries: {}", forge.parent().unwrap().display());
    if which("forge").is_none() {
        println!();
        println!("  Note: ~/.foundry/bin is not on the current shell's PATH.");
        println!("  Open a new terminal, or run: export PATH=\"$HOME/.foundry/bin:$PATH\"");
    }
    println!();
    println!("Next: cd contracts && forge install foundry-rs/forge-std openzeppelin/openzeppelin-contracts");
    Ok(())
}

fn which(cmd: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(cmd))
            .find(|p| p.is_file())
    })
}

pub fn uninstall(only_claude: bool, only_codex: bool, only_cursor: bool) -> Result<()> {
    let home = dirs::home_dir().context("could not determine $HOME")?;

    let auto = !only_claude && !only_codex && !only_cursor;
    let do_claude = only_claude || (auto && home.join(".claude").exists());
    let do_codex  = only_codex  || (auto && home.join(".codex").exists());
    let do_cursor = only_cursor || (auto && home.join(".cursor").exists());

    if !do_claude && !do_codex && !do_cursor {
        println!("No supported AI tools detected. Nothing to uninstall.");
        return Ok(());
    }

    if do_claude {
        uninstall_claude(&home)?;
    }
    if do_codex {
        uninstall_codex(&home)?;
    }
    if do_cursor {
        uninstall_cursor(&home)?;
    }

    Ok(())
}

fn uninstall_claude(home: &std::path::Path) -> Result<()> {
    let settings_path = home.join(".claude").join("settings.json");
    if !settings_path.exists() {
        println!("✓ Claude Code: nothing to remove ({} not found)", settings_path.display());
        return Ok(());
    }

    let text = std::fs::read_to_string(&settings_path)
        .with_context(|| format!("reading {}", settings_path.display()))?;
    let mut settings: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", settings_path.display()))?;

    let mut removed = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PreToolUse"))
        .and_then(|p| p.as_array_mut())
        .map(|pretool| {
            let mut removed = 0usize;
            for entry in pretool.iter_mut() {
                if let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                    let before = hooks.len();
                    hooks.retain(|h| {
                        let cmd = h.get("command").and_then(|c| c.as_str()).unwrap_or("");
                        !(cmd.contains("tkr") && cmd.contains("hook claude"))
                    });
                    removed += before - hooks.len();
                }
            }
            removed
        })
        .unwrap_or(0);

    // Also strip any tkr PostToolUse entries so uninstall is symmetric.
    removed += settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PostToolUse"))
        .and_then(|p| p.as_array_mut())
        .map(|posttool| {
            let mut removed = 0usize;
            for entry in posttool.iter_mut() {
                if let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
                    let before = hooks.len();
                    hooks.retain(|h| {
                        let cmd = h.get("command").and_then(|c| c.as_str()).unwrap_or("");
                        !(cmd.contains("tkr") && cmd.contains("hook post"))
                    });
                    removed += before - hooks.len();
                }
            }
            removed
        })
        .unwrap_or(0);

    let mcp_removed = settings
        .get_mut("mcpServers")
        .and_then(|s| s.as_object_mut())
        .map(|servers| servers.remove("tkr").is_some())
        .unwrap_or(false);
    if mcp_removed {
        removed += 1;
    }

    // Also strip the CLAUDE.md include line + the tkr.md fragment.
    let fragment = home.join(TKR_FRAGMENT_PATH);
    if fragment.exists() {
        let _ = std::fs::remove_file(&fragment);
        removed += 1;
    }
    let main_md = home.join(".claude").join("CLAUDE.md");
    if let Ok(existing) = std::fs::read_to_string(&main_md) {
        let cleaned: String = existing
            .lines()
            .filter(|l| l.trim() != "@tkr.md")
            .collect::<Vec<_>>()
            .join("\n");
        if cleaned != existing {
            let _ = std::fs::write(&main_md, cleaned + "\n");
            removed += 1;
        }
    }

    if removed == 0 {
        println!("✓ Claude Code: tkr hook not present in {}", settings_path.display());
        return Ok(());
    }

    let serialized = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, serialized + "\n")
        .with_context(|| format!("writing {}", settings_path.display()))?;
    println!("✓ Claude Code: removed tkr hook from {}", settings_path.display());
    Ok(())
}

fn uninstall_cursor(home: &std::path::Path) -> Result<()> {
    let rule_path = home.join(CURSOR_RULE_PATH);
    if !rule_path.exists() {
        println!("✓ Cursor: nothing to remove ({} not found)", rule_path.display());
        return Ok(());
    }
    std::fs::remove_file(&rule_path)
        .with_context(|| format!("removing {}", rule_path.display()))?;
    println!("✓ Cursor: removed {}", rule_path.display());
    Ok(())
}

fn uninstall_codex(home: &std::path::Path) -> Result<()> {
    let config_path = home.join(".codex").join("config.toml");
    if !config_path.exists() {
        println!("✓ Codex: nothing to remove ({} not found)", config_path.display());
        return Ok(());
    }

    let text = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;

    // Remove every `[[hooks.PreToolUse]]` block whose body mentions both
    // "tkr" and "hook claude". A block runs from its `[[hooks.PreToolUse]]`
    // header up to (but not including) the next top-level `[…]`/`[[…]]`
    // header or end of file.
    let mut out = String::with_capacity(text.len());
    let mut iter = text.lines().peekable();
    let mut removed = 0usize;
    while let Some(line) = iter.next() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("[[hooks.PreToolUse]]") {
            let mut block = String::from(line);
            block.push('\n');
            while let Some(next) = iter.peek() {
                let nt = next.trim_start();
                let starts_new_top = nt.starts_with("[[") && !nt.starts_with("[[hooks.PreToolUse.hooks");
                let starts_new_section = nt.starts_with('[') && !nt.starts_with("[[") && !nt.starts_with("[hooks.");
                if starts_new_top || starts_new_section {
                    break;
                }
                block.push_str(iter.next().unwrap());
                block.push('\n');
            }
            if block.contains("tkr") && block.contains("hook claude") {
                removed += 1;
                continue;
            }
            out.push_str(&block);
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    if removed == 0 {
        println!("✓ Codex: tkr hook not present in {}", config_path.display());
        return Ok(());
    }

    std::fs::write(&config_path, out)
        .with_context(|| format!("writing {}", config_path.display()))?;
    println!("✓ Codex: removed tkr hook from {}", config_path.display());
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

    let existing_idx = bash_hooks.iter().position(|h| {
        h.get("command").and_then(|c| c.as_str())
            .map(|s| s.contains("tkr") && s.contains("hook claude"))
            .unwrap_or(false)
    });

    let already_present = match existing_idx {
        Some(i) => {
            let current = bash_hooks[i].get("command").and_then(|c| c.as_str()).unwrap_or("");
            if current != hook_command {
                // Path changed (e.g. user upgraded brew → /opt/homebrew/bin/tkr).
                // Update in place so the hook always points at the live binary.
                bash_hooks[i] = json!({ "type": "command", "command": hook_command.clone() });
            }
            true
        }
        None => {
            bash_hooks.push(json!({ "type": "command", "command": hook_command.clone() }));
            false
        }
    };

    // PostToolUse hook for Read/Grep/Glob — adds steering notes for
    // oversized tool results (Phase 1 of MCP migration).
    ensure_post_hook(root, bin)?;

    // MCP server registration — exposes tkr_outline_file, tkr_find_symbol,
    // tkr_grep_summary so the model can opt into structured summaries.
    ensure_mcp_server(root, bin)?;

    // CLAUDE.md fragment nudging the model to prefer tkr's MCP tools for
    // large files / wide patterns. Writes a separate file so the user's
    // top-level CLAUDE.md stays untouched.
    write_claude_md_fragment(home)?;

    let serialized = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&settings_path, serialized + "\n")
        .with_context(|| format!("writing {}", settings_path.display()))?;

    if already_present {
        println!(
            "✓ Claude Code: refreshed at {} (PreToolUse + PostToolUse + MCP + CLAUDE.md fragment)",
            settings_path.display()
        );
    } else {
        println!(
            "✓ Claude Code: installed into {}\n  Restart Claude Code to activate.",
            settings_path.display()
        );
    }
    Ok(())
}

/// Ensure `hooks.PostToolUse` contains a tkr hook entry matching
/// Read|Grep|Glob. Idempotent: updates the command if a tkr post-hook
/// already exists, otherwise appends. Operates on the already-mutable
/// settings root (`root` is `&mut serde_json::Map`).
fn ensure_post_hook(
    root: &mut serde_json::Map<String, Value>,
    bin: &str,
) -> Result<()> {
    let hook_command = format!("{bin} hook post");
    let matcher = "Read|Grep|Glob";

    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("hooks must be a JSON object")?;
    let posttool = hooks
        .entry("PostToolUse")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("hooks.PostToolUse must be an array")?;

    // Find or create the matcher block.
    let block_idx = posttool.iter().position(|m| {
        m.get("matcher")
            .and_then(|x| x.as_str())
            .map(|s| s == matcher)
            .unwrap_or(false)
    });
    let block = match block_idx {
        Some(i) => &mut posttool[i],
        None => {
            posttool.push(json!({ "matcher": matcher, "hooks": [] }));
            posttool.last_mut().unwrap()
        }
    };
    let block_hooks = block
        .as_object_mut()
        .context("PostToolUse matcher must be an object")?
        .entry("hooks")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("PostToolUse hooks must be an array")?;

    // Find an existing tkr post-hook (any binary path) to update in place.
    let existing_idx = block_hooks.iter().position(|h| {
        h.get("command")
            .and_then(|c| c.as_str())
            .map(|s| s.contains("tkr") && s.contains("hook post"))
            .unwrap_or(false)
    });

    match existing_idx {
        Some(i) => {
            block_hooks[i] = json!({ "type": "command", "command": hook_command });
        }
        None => {
            block_hooks.push(json!({ "type": "command", "command": hook_command }));
        }
    }
    Ok(())
}

/// CLAUDE.md fragment dropped into `~/.claude/tkr.md` and referenced
/// from the user's main `~/.claude/CLAUDE.md` (only if not already
/// referenced). Steers the model toward tkr's MCP tools for the cases
/// where they meaningfully reduce token cost.
const TKR_FRAGMENT_PATH: &str = ".claude/tkr.md";
const TKR_FRAGMENT_CONTENT: &str = r#"# tkr — token-saving tool selection

When the `tkr` MCP server is available, prefer its tools over native
`Read` / `Grep` / `Glob` in these cases:

| Situation                                              | Use                              |
|--------------------------------------------------------|----------------------------------|
| Need to know what's in a file > ~200 lines             | `tkr_outline_file`               |
| Looking for a specific symbol's definition             | `tkr_find_symbol`                |
| Recursive grep that may match across many files        | `tkr_grep_summary`               |
| Need actual line ranges of a function before editing   | `tkr_outline_file` then native `Read` with `offset`/`limit` |

Native `Read` is fine for small files (< 200 lines) and for the exact
ranges you've already pinpointed.

Native `Grep` is fine for narrow searches with `path` / `type` /
`head_limit` already constraining the result set.

These are guidelines, not hard rules — pick the tool that lets you
answer the actual question with the fewest tokens.
"#;

fn write_claude_md_fragment(home: &std::path::Path) -> Result<()> {
    let fragment = home.join(TKR_FRAGMENT_PATH);
    if let Some(parent) = fragment.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&fragment, TKR_FRAGMENT_CONTENT)
        .with_context(|| format!("writing {}", fragment.display()))?;

    // Reference it from the user's main CLAUDE.md if there is one. We
    // only ADD a one-line "@tkr.md" include; we don't modify any existing
    // content. Skip if the line is already present, or if there is no
    // CLAUDE.md to extend.
    let main_md = home.join(".claude").join("CLAUDE.md");
    let include_line = "@tkr.md";
    let existing = std::fs::read_to_string(&main_md).unwrap_or_default();
    if existing.contains(include_line) {
        return Ok(());
    }
    let mut new_contents = existing;
    if !new_contents.is_empty() && !new_contents.ends_with('\n') {
        new_contents.push('\n');
    }
    new_contents.push_str("\n");
    new_contents.push_str(include_line);
    new_contents.push('\n');
    std::fs::write(&main_md, new_contents)
        .with_context(|| format!("writing {}", main_md.display()))?;
    Ok(())
}

/// Ensure `mcpServers.tkr` is registered in `~/.claude/settings.json`,
/// pointing at `<bin> mcp`. Claude Code will spawn this on startup and
/// expose its tools (tkr_outline_file, tkr_find_symbol, tkr_grep_summary)
/// to the model.
fn ensure_mcp_server(
    root: &mut serde_json::Map<String, Value>,
    bin: &str,
) -> Result<()> {
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("mcpServers must be a JSON object")?;

    servers.insert(
        "tkr".to_string(),
        json!({
            "command": bin,
            "args": ["mcp"]
        }),
    );
    Ok(())
}

// ── Cursor ────────────────────────────────────────────────────────────────────
//
// Cursor doesn't have a PreToolUse bash hook. Instead we write a Cursor rule
// (~/.cursor/rules/tkr.mdc with alwaysApply: true) that instructs the AI to
// always prefix shell commands with `tkr`. This is prompt-level, not OS-level.

const CURSOR_RULE_PATH: &str = ".cursor/rules/tkr.mdc";

fn install_cursor(home: &std::path::Path) -> Result<()> {
    let rule_path = home.join(CURSOR_RULE_PATH);

    if rule_path.exists() {
        println!("✓ Cursor: already installed at {}", rule_path.display());
        return Ok(());
    }

    if let Some(parent) = rule_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    let rule = "\
---
description: tkr token-optimization proxy — prefix all shell commands with tkr
alwaysApply: true
---

## tkr — Token-Optimized Commands

Always prefix shell commands with `tkr` to reduce token usage by 60-90%.
`tkr` is transparent: if no filter exists for a command it passes through unchanged.

### Examples

```bash
# Git
tkr git status
tkr git diff
tkr git log

# Cargo / Rust
tkr cargo build
tkr cargo test
tkr cargo check
tkr cargo clippy

# Node / JS
tkr npm run dev
tkr pnpm install

# Files & Search
tkr ls src/
tkr grep \"fn main\" .

# Docker / K8s
tkr docker ps
tkr kubectl get pods
```

### Rule

When generating shell commands to run in the terminal, **always prepend `tkr`**.
This applies to: git, cargo, npm, pnpm, yarn, docker, kubectl, make, grep, find, ls, curl.
";

    std::fs::write(&rule_path, rule)
        .with_context(|| format!("writing {}", rule_path.display()))?;

    println!(
        "✓ Cursor: installed rule at {}\n  Cursor Agent will now prefix commands with tkr.",
        rule_path.display()
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
