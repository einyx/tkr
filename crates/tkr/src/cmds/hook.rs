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
//! Always fails open: any error path returns Ok(()) with no output so the
//! host tool never breaks.

use anyhow::Result;
use serde_json::{json, Value};
use std::io::{Read, Write};

/// Hard cap on hook stdin payload to bound memory in case a producer
/// streams unbounded data.
const MAX_PAYLOAD_BYTES: u64 = 1_048_576;

pub fn run_claude() -> Result<()> {
    process_claude(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())
}

fn process_claude<R: Read, W: Write>(input: &mut R, output: &mut W) -> Result<()> {
    let mut buf = String::new();
    let _ = input.take(MAX_PAYLOAD_BYTES).read_to_string(&mut buf);
    if buf.trim().is_empty() {
        return Ok(());
    }

    let payload: Value = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let cmd = payload
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if cmd.is_empty() {
        return Ok(());
    }

    let rewritten = match crate::cmds::rewrite::try_rewrite(cmd) {
        Some(r) => r,
        None => return Ok(()),
    };
    if rewritten == cmd || rewritten == cmd.trim_start() {
        return Ok(());
    }

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
    let _ = writeln!(output, "{}", response);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> String {
        let mut out: Vec<u8> = Vec::new();
        process_claude(&mut input.as_bytes(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn empty_input_no_output() {
        assert_eq!(run(""), "");
        assert_eq!(run("   \n"), "");
    }

    #[test]
    fn malformed_json_no_output() {
        assert_eq!(run("{not json"), "");
    }

    #[test]
    fn missing_command_no_output() {
        assert_eq!(run(r#"{"tool_input":{}}"#), "");
        assert_eq!(run(r#"{}"#), "");
    }

    #[test]
    fn empty_command_no_output() {
        assert_eq!(run(r#"{"tool_input":{"command":""}}"#), "");
    }

    #[test]
    fn unknown_command_no_output() {
        assert_eq!(run(r#"{"tool_input":{"command":"echo hi"}}"#), "");
    }

    #[test]
    fn already_tkr_no_output() {
        assert_eq!(run(r#"{"tool_input":{"command":"tkr git status"}}"#), "");
    }

    #[test]
    fn known_command_emits_hook_response() {
        let out = run(r#"{"tool_input":{"command":"git status"}}"#);
        assert!(out.contains("\"permissionDecision\":\"allow\""));
        assert!(out.contains("\"command\":\"tkr git status\""));
    }

    #[test]
    fn preserves_other_tool_input_fields() {
        let out = run(r#"{"tool_input":{"command":"git status","timeout":5000,"description":"check"}}"#);
        assert!(out.contains("\"timeout\":5000"));
        assert!(out.contains("\"description\":\"check\""));
        assert!(out.contains("\"command\":\"tkr git status\""));
    }

    #[test]
    fn compound_command_each_segment_prefixed() {
        let out = run(r#"{"tool_input":{"command":"git add . && git commit -m hi"}}"#);
        assert!(out.contains("\"command\":\"tkr git add . && tkr git commit -m hi\""));
    }

    #[test]
    fn backticks_bail_out_no_output() {
        let out = run(r#"{"tool_input":{"command":"git status `echo &&`"}}"#);
        // Backticks present → no rewrite emitted.
        assert_eq!(out, "");
    }

    #[test]
    fn enormous_input_bounded() {
        // Construct a payload larger than the cap. Should not panic / exhaust.
        let big = "x".repeat((MAX_PAYLOAD_BYTES as usize) + 10_000);
        let payload = format!(r#"{{"tool_input":{{"command":"{big}"}}}}"#);
        // Truncated read will produce malformed JSON → fail-open, no output.
        let _ = run(&payload);
    }
}
