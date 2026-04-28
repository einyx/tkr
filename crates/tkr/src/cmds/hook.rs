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
    std::io::stdin().read_to_string(&mut input)?;
    if input.trim().is_empty() {
        return Ok(());
    }

    let payload: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return Ok(()), // bad JSON → don't break the host
    };

    let cmd = payload
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if cmd.is_empty() {
        return Ok(());
    }

    if let Some(rewritten) = crate::cmds::rewrite::try_rewrite(cmd) {
        if rewritten == cmd {
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
    }

    Ok(())
}
