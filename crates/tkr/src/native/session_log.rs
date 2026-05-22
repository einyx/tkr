//! Optional JSONL trail when `TKR_NATIVE_SESSION_LOG=1` — lightweight “replay” of
//! native-handler runs (command, sizes, exit). One line per invocation.

use crate::stream::PipelineResult;
use serde_json::json;
use std::io::Write;

pub fn maybe_append(cmd: &str, args: &[String], pipeline: &PipelineResult, exit_code: i32) {
    if std::env::var("TKR_NATIVE_SESSION_LOG").ok().as_deref() != Some("1") {
        return;
    }

    let home = dirs::home_dir().unwrap_or_default();
    let path = home.join(".tkr/native-handlers.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let row = json!({
        "ts": ts,
        "cmd": cmd,
        "args": args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "chars_in": pipeline.chars_in,
        "chars_saved": pipeline.chars_suppressed,
        "exit": exit_code,
    });

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{}", row);
    }
}
