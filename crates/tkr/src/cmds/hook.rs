//! `tkr hook claude` — Claude Code PreToolUse Bash hook.
//!
//! `tkr hook universal` — same JSON response shape; accepts either Claude's
//! `tool_input.command` or a top-level `"command"` field (shell / IDE wrappers).
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
    process_hook(
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        extract_command_claude,
    )
}

pub fn run_universal() -> Result<()> {
    process_hook(
        &mut std::io::stdin().lock(),
        &mut std::io::stdout().lock(),
        extract_command_universal,
    )
}

fn extract_command_claude(payload: &Value) -> Option<&str> {
    payload
        .get("tool_input")
        .and_then(|t| t.get("command"))
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
}

fn extract_command_universal(payload: &Value) -> Option<&str> {
    extract_command_claude(payload).or_else(|| {
        payload
            .get("command")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
    })
}

fn process_hook<R: Read, W: Write, F>(input: &mut R, output: &mut W, extract: F) -> Result<()>
where
    F: Fn(&Value) -> Option<&str>,
{
    let mut buf = String::new();
    let _ = input.take(MAX_PAYLOAD_BYTES).read_to_string(&mut buf);
    if buf.trim().is_empty() {
        return Ok(());
    }

    let payload: Value = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let cmd = extract(&payload).unwrap_or("");
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
    let _ = writeln!(output, "{response}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> String {
        let mut out: Vec<u8> = Vec::new();
        process_hook(&mut input.as_bytes(), &mut out, extract_command_claude).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn run_universal(input: &str) -> String {
        let mut out: Vec<u8> = Vec::new();
        process_hook(&mut input.as_bytes(), &mut out, extract_command_universal).unwrap();
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
        let out =
            run(r#"{"tool_input":{"command":"git status","timeout":5000,"description":"check"}}"#);
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

    #[test]
    fn universal_top_level_command_rewrites() {
        let out = run_universal(r#"{"command":"git status"}"#);
        assert!(out.contains("\"command\":\"tkr git status\""));
    }

    #[test]
    fn universal_prefers_tool_input_over_top_level() {
        let out = run_universal(r#"{"tool_input":{"command":"git status"},"command":"echo noop"}"#);
        assert!(out.contains("\"command\":\"tkr git status\""));
    }
}
