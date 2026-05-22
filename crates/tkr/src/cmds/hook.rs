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

// ---------- PostToolUse hook ----------

/// PostToolUse hook. Reads a payload like:
///
///   {
///     "session_id": "...",
///     "tool_name": "Read" | "Grep" | "Glob" | "Edit" | "Bash" | ...,
///     "tool_input": {...},
///     "tool_response": "..." | {...}
///   }
///
/// Records per-tool size analytics (so `tkr gain` covers the WHOLE
/// context spend, not just Bash) and emits a steering note via
/// `hookSpecificOutput.additionalContext` when a result was likely too
/// large. Cannot rewrite the result that already entered the LLM's
/// context — that requires an MCP wrapper (Phase 2).
///
/// Always fails open. Empty stdout = no steering note.
pub fn run_post() -> Result<()> {
    process_post(&mut std::io::stdin().lock(), &mut std::io::stdout().lock())
}

fn process_post<R: Read, W: Write>(input: &mut R, output: &mut W) -> Result<()> {
    let mut buf = String::new();
    let _ = input.take(MAX_PAYLOAD_BYTES).read_to_string(&mut buf);
    if buf.trim().is_empty() {
        return Ok(());
    }
    let payload: Value = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let tool = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if tool.is_empty() {
        return Ok(());
    }
    let response_chars = payload
        .get("tool_response")
        .map(response_size_chars)
        .unwrap_or(0);
    let approx_tokens = response_chars / 4;

    // (analytics recording deliberately omitted in this slice — analytics
    // crate's API is per-Bash-command oriented; a tool-spend table is a
    // small follow-up.)

    let note = steering_note(tool, &payload, approx_tokens);
    if note.is_empty() {
        return Ok(());
    }

    let response = json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": note,
        }
    });
    let _ = writeln!(output, "{response}");
    Ok(())
}

/// Estimate the byte size of a tool_response. Claude Code emits this as
/// either a JSON string, an object with a `text` / `output` / `result`
/// field, or sometimes a raw string. We pick the longest plausible
/// content-bearing field; if none, fall back to the JSON serialization
/// length (worst case).
fn response_size_chars(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::Object(map) => {
            for k in &["text", "output", "result", "content", "stdout"] {
                if let Some(v) = map.get(*k) {
                    if let Some(s) = v.as_str() {
                        return s.len();
                    }
                }
            }
            // Fallback: serialize the whole object.
            value.to_string().len()
        }
        other => other.to_string().len(),
    }
}

/// Decide whether to emit a steering note, and what to say. Empty string
/// = no note (silent pass-through). Notes are short — they get inlined
/// into the next-turn context and we don't want to spend tokens nagging
/// the model.
fn steering_note(tool: &str, payload: &Value, approx_tokens: usize) -> String {
    // Thresholds chosen so we only nudge on genuinely-large results.
    // Large = ~2K tokens (~8KB) for Read/Grep/Glob; Bash already has
    // its own filter pipeline so we stay quiet there.
    let large_threshold_tokens = 2_000;
    if approx_tokens < large_threshold_tokens {
        return String::new();
    }

    match tool {
        "Read" => {
            let path = payload
                .get("tool_input")
                .and_then(|v| v.get("file_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            format!(
                "[tkr] that Read returned ~{}K tokens from {}. \
                 Next time, prefer narrowing with offset/limit, or \
                 (when available) the tkr_outline / tkr_grep MCP tools \
                 for a structured summary.",
                approx_tokens / 1000,
                path
            )
        }
        "Grep" => {
            let pattern = payload
                .get("tool_input")
                .and_then(|v| v.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)");
            format!(
                "[tkr] that Grep for {pattern:?} returned ~{}K tokens. \
                 Consider using head_limit / type / path filters, or \
                 (when available) the tkr_find_symbol MCP tool.",
                approx_tokens / 1000
            )
        }
        "Glob" => format!(
            "[tkr] that Glob returned ~{}K tokens. Consider tighter \
             patterns or path scoping.",
            approx_tokens / 1000
        ),
        // Bash has its own filter pipeline — don't double up.
        _ => String::new(),
    }
}

#[cfg(test)]
mod post_tests {
    use super::*;

    fn run_post(input: &str) -> String {
        let mut out = Vec::new();
        process_post(&mut input.as_bytes(), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn empty_stdin_is_noop() {
        assert_eq!(run_post(""), "");
    }

    #[test]
    fn small_read_response_is_silent() {
        // ~1KB content, well below the 2K-token threshold.
        let payload = json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/tmp/x.rs"},
            "tool_response": {"text": "x".repeat(1_000)},
        })
        .to_string();
        assert_eq!(run_post(&payload), "");
    }

    #[test]
    fn large_read_emits_steering_note() {
        // ~12KB content, ~3K tokens.
        let payload = json!({
            "tool_name": "Read",
            "tool_input": {"file_path": "/src/big.rs"},
            "tool_response": {"text": "x".repeat(12_000)},
        })
        .to_string();
        let out = run_post(&payload);
        assert!(out.contains("Read returned"), "out: {out}");
        assert!(out.contains("/src/big.rs"));
        assert!(out.contains("hookSpecificOutput"));
        assert!(out.contains("PostToolUse"));
    }

    #[test]
    fn bash_is_silent_postuse() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": {"command": "git status"},
            "tool_response": "x".repeat(20_000),
        })
        .to_string();
        // Bash has its own pipeline; PostToolUse stays out of its way.
        assert_eq!(run_post(&payload), "");
    }

    #[test]
    fn malformed_payload_is_silent() {
        assert_eq!(run_post("not-json"), "");
        assert_eq!(run_post("{\"tool_name\": null}"), "");
    }

    #[test]
    fn response_size_reads_text_field_in_object() {
        let v = json!({"text": "hello"});
        assert_eq!(response_size_chars(&v), 5);
    }

    #[test]
    fn response_size_reads_string_directly() {
        let v = json!("plain string");
        assert_eq!(response_size_chars(&v), 12);
    }
}
