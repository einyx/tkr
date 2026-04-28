//! `tkr hook claude` — Claude Code PreToolUse Bash hook.
//!
//! Reads a JSON object from stdin like:
//!   {"tool_input": {"command": "git status"}}
//!
//! If the command can be rewritten with tkr, emits:
//!   {"hookSpecificOutput": {
//!     "hookEventName": "PreToolUse",
//!     "permissionDecision": "allow",
//!     "permissionDecisionReason": "tkr auto-rewrite (token filter)",
//!     "updatedInput": {"command": "tkr git status"}
//!   }}
//!
//! Otherwise exits silently (exit 0, no output) → command passes through unchanged.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;

pub fn run_claude() -> Result<()> {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("tkr hook: stdin read failed: {e}");
        return Ok(());
    }
    if input.trim().is_empty() {
        return Ok(());
    }

    let payload: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("tkr hook: bad JSON ({e})");
            return Ok(());
        }
    };

    let cmd = payload
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if cmd.is_empty() {
        return Ok(());
    }

    let Some(rewritten) = crate::cmds::rewrite::try_rewrite(cmd) else {
        return Ok(());
    };
    // Compare against trimmed input — try_rewrite returns the trimmed form
    // even for already-prefixed commands, and we don't want a whitespace-only
    // change to fire the hook.
    if rewritten == cmd.trim_start() {
        return Ok(());
    }

    // Build response: copy original tool_input, override command.
    let mut updated = payload
        .get("tool_input")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(obj) = updated.as_object_mut() {
        obj.insert("command".into(), Value::String(rewritten));
    }
    let response = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "tkr auto-rewrite (token filter)",
            "updatedInput": updated
        }
    });
    println!("{}", response);
    Ok(())
}
