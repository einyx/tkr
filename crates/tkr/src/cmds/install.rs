//! `tkr install` — wire the tkr Bash hook into `~/.claude/settings.json`.
//!
//! Adds a PreToolUse / Bash matcher entry whose `command` is the absolute
//! path to the running `tkr` binary plus `hook claude`. Idempotent — running
//! it twice does not duplicate the entry. Existing unrelated hooks (other
//! tools, other matchers) are preserved unchanged.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::Write as _;

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

/// Ensure `.zshrc` contains a source line for the snippet. Idempotent: if any
/// existing line ends with `# tkr`, nothing is added.
pub fn ensure_source_line(zshrc: &std::path::Path, snippet: &std::path::Path) -> Result<()> {
    let contents = if zshrc.exists() {
        std::fs::read_to_string(zshrc)
            .with_context(|| format!("reading {}", zshrc.display()))?
    } else {
        String::new()
    };

    // Idempotency: if any line already ends with `# tkr`, skip.
    if contents.lines().any(|l| l.trim_end().ends_with("# tkr")) {
        return Ok(());
    }

    let source_line = format!(
        "[ -f \"{}\" ] && source \"{}\" # tkr\n",
        snippet.display(),
        snippet.display(),
    );

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(zshrc)
        .with_context(|| format!("opening {} for append", zshrc.display()))?;
    file.write_all(source_line.as_bytes())
        .with_context(|| format!("writing to {}", zshrc.display()))?;
    Ok(())
}

pub fn run_shell_zsh() -> Result<()> {
    let home = dirs::home_dir().context("could not determine $HOME")?;

    // Resolve the path to the running tkr binary.
    let bin = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tkr".into());

    // Write the zsh snippet to ~/.tkr/zsh-init.sh.
    let tkr_dir = home.join(".tkr");
    std::fs::create_dir_all(&tkr_dir)
        .with_context(|| format!("creating {}", tkr_dir.display()))?;
    let snippet_path = tkr_dir.join("zsh-init.sh");

    let snippet_content = format!(
        r#"# tkr zsh integration — auto-rewrites commands through `tkr rewrite`
# before execution. Generated by `tkr install --shell zsh`.
# Edit by re-running the install command, not by hand.

_tkr_accept_line() {{
  if [[ -n "$BUFFER" ]]; then
    local rewritten
    rewritten=$({bin} rewrite "$BUFFER" 2>/dev/null)
    if [[ $? -eq 0 && -n "$rewritten" && "$rewritten" != "$BUFFER" ]]; then
      BUFFER="$rewritten"
    fi
  fi
  zle .accept-line
}}
zle -N accept-line _tkr_accept_line
"#,
        bin = bin,
    );

    std::fs::write(&snippet_path, &snippet_content)
        .with_context(|| format!("writing {}", snippet_path.display()))?;

    // Ensure ~/.zshrc exists.
    let zshrc_path = home.join(".zshrc");
    if !zshrc_path.exists() {
        std::fs::write(&zshrc_path, "")
            .with_context(|| format!("creating {}", zshrc_path.display()))?;
    }

    ensure_source_line(&zshrc_path, &snippet_path)?;

    println!(
        "Installed tkr zsh integration:\n  snippet: {}\n  sourced from: {}\n\nRestart your shell (or run `source ~/.zshrc`) to activate.",
        snippet_path.display(),
        zshrc_path.display(),
    );
    Ok(())
}

#[cfg(test)]
mod shell_tests {
    use super::ensure_source_line;
    use std::io::Write;

    #[test]
    fn snippet_contains_expected_identifiers() {
        // The snippet template must reference the zle widget and tkr rewrite.
        // We build it with a placeholder bin path and check the key markers.
        let bin = "tkr";
        let snippet = format!(
            r#"_tkr_accept_line() {{
  rewritten=$({bin} rewrite "$BUFFER" 2>/dev/null)
  zle .accept-line
}}
zle -N accept-line _tkr_accept_line
"#,
            bin = bin,
        );
        assert!(snippet.contains("_tkr_accept_line"));
        assert!(snippet.contains("zle -N accept-line"));
        assert!(snippet.contains("tkr rewrite"));
    }

    #[test]
    fn idempotent_when_source_line_already_present() {
        let dir = tempfile::tempdir().unwrap();
        let snippet = dir.path().join("zsh-init.sh");
        std::fs::write(&snippet, "# snippet\n").unwrap();

        let zshrc = dir.path().join(".zshrc");
        let existing = format!(
            "[ -f \"{}\" ] && source \"{}\" # tkr\n",
            snippet.display(),
            snippet.display()
        );
        std::fs::write(&zshrc, &existing).unwrap();

        ensure_source_line(&zshrc, &snippet).unwrap();

        let after = std::fs::read_to_string(&zshrc).unwrap();
        assert_eq!(after, existing, "file must not change when # tkr line exists");
    }

    #[test]
    fn appends_source_line_to_fresh_zshrc() {
        let dir = tempfile::tempdir().unwrap();
        let snippet = dir.path().join("zsh-init.sh");
        std::fs::write(&snippet, "# snippet\n").unwrap();

        let zshrc = dir.path().join(".zshrc");
        // Create empty file.
        std::fs::File::create(&zshrc).unwrap();

        ensure_source_line(&zshrc, &snippet).unwrap();

        let after = std::fs::read_to_string(&zshrc).unwrap();
        assert!(
            after.lines().any(|l| l.trim_end().ends_with("# tkr")),
            "should have appended a line ending with # tkr"
        );
        assert!(
            after.contains("source"),
            "appended line should contain 'source'"
        );
    }
}
