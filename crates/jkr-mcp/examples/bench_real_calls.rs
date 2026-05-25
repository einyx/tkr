//! Replay a corpus of real MCP tool calls and measure per-call byte cost.
//!
//! Reads a JSONL corpus where each line is `{"tool": "...", "args": {...}}`,
//! re-runs each call through the index-backed handlers in jkr-mcp, and
//! reports the actual byte count per tool. This is the honest measurement
//! the hand-picked `bench_token_cost` was a proxy for.
//!
//! Build a corpus from your own agent transcripts:
//!
//!   cd ~/.claude/projects/<your-project>
//!   <extract tool_use blocks where name starts with mcp__jkr__,
//!    emit one JSON object per line with `tool` and `args`>
//!
//! Then:
//!
//!   cargo run -p jkr-mcp --release --example bench_real_calls -- corpus.jsonl
//!
//! If the corpus is empty or tiny, that *is* the signal — there's no
//! adoption to optimize for yet.
//!
//! Usage:
//!   cargo run -p jkr-mcp --release --example bench_real_calls -- <corpus.jsonl>

use anyhow::{bail, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use jkr_mcp::{index_backed, outline};

#[derive(serde::Deserialize)]
struct Call {
    tool: String,
    args: serde_json::Value,
}

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}
fn arg_u64(args: &serde_json::Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}
fn arg_bool(args: &serde_json::Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}

fn dispatch(call: &Call) -> Result<Option<String>> {
    let root = match arg_str(&call.args, "root") {
        Some(s) => PathBuf::from(s),
        None => std::env::current_dir()?,
    };
    match call.tool.as_str() {
        "jkr_outline_file" => {
            let path = arg_str(&call.args, "path")
                .ok_or_else(|| anyhow::anyhow!("missing path"))?;
            let p = PathBuf::from(path);
            // Real transcripts have at least one call that passed a directory
            // — keep this behavior visible (the handler errors and the bench
            // reports zero bytes), don't paper over it.
            if !p.is_file() {
                eprintln!("[skip] {} → not a file: {}", call.tool, path);
                return Ok(Some(String::new()));
            }
            outline::render_outline(&p).map(Some)
        }
        "jkr_find_symbol" => {
            let name = arg_str(&call.args, "name").ok_or_else(|| anyhow::anyhow!("missing name"))?;
            index_backed::try_find_symbol(name, &root)
        }
        "jkr_signature" => {
            let name = arg_str(&call.args, "name").ok_or_else(|| anyhow::anyhow!("missing name"))?;
            index_backed::try_signature(name, &root)
        }
        "jkr_read_smart" => {
            let q = arg_str(&call.args, "question")
                .ok_or_else(|| anyhow::anyhow!("missing question"))?;
            let limit = arg_u64(&call.args, "limit").unwrap_or(8) as usize;
            let verbose = arg_bool(&call.args, "verbose").unwrap_or(false);
            index_backed::try_read_smart(q, &root, limit, verbose)
        }
        "jkr_callers_of" => {
            let name = arg_str(&call.args, "name").ok_or_else(|| anyhow::anyhow!("missing name"))?;
            index_backed::try_callers_of(name, &root)
        }
        "jkr_callees_of" => {
            let name = arg_str(&call.args, "name").ok_or_else(|| anyhow::anyhow!("missing name"))?;
            index_backed::try_callees_of(name, &root)
        }
        "jkr_call_path" => {
            let from = arg_str(&call.args, "from")
                .ok_or_else(|| anyhow::anyhow!("missing from"))?;
            let to = arg_str(&call.args, "to").ok_or_else(|| anyhow::anyhow!("missing to"))?;
            let depth = arg_u64(&call.args, "max_depth").unwrap_or(6) as usize;
            index_backed::try_call_path(from, to, depth, &root)
        }
        // grep_summary and jobs_list / mesh_status live in other modules and
        // aren't measured here yet. The corpus reveals which ones are real
        // pressure points — fold them in once they appear with enough volume.
        other => {
            eprintln!("[unhandled] {other}");
            Ok(Some(String::new()))
        }
    }
}

fn main() -> Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "corpus.jsonl".to_string());
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => bail!("open {}: {e}", path),
    };
    let mut per_tool: HashMap<String, (usize, usize)> = HashMap::new(); // tool → (calls, bytes)
    let mut total_bytes = 0usize;
    let mut total_calls = 0usize;

    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let call: Call = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[skip] bad line: {e}");
                continue;
            }
        };
        let out = match dispatch(&call) {
            Ok(Some(s)) => s,
            Ok(None) => {
                eprintln!("[skip] {}: no index at root", call.tool);
                continue;
            }
            Err(e) => {
                eprintln!("[err] {}: {e}", call.tool);
                continue;
            }
        };
        let entry = per_tool.entry(call.tool.clone()).or_default();
        entry.0 += 1;
        entry.1 += out.len();
        total_calls += 1;
        total_bytes += out.len();
    }

    println!();
    println!("{:<22} {:>6} {:>10} {:>10}", "tool", "calls", "bytes", "B/call");
    println!("{}", "-".repeat(50));
    let mut tools: Vec<_> = per_tool.iter().collect();
    tools.sort_by_key(|(_, (_, b))| std::cmp::Reverse(*b));
    for (tool, (calls, bytes)) in tools {
        let per = if *calls > 0 { *bytes / *calls } else { 0 };
        println!("{:<22} {:>6} {:>10} {:>10}", tool, calls, bytes, per);
    }
    println!("{}", "-".repeat(50));
    println!("{:<22} {:>6} {:>10}", "TOTAL", total_calls, total_bytes);
    if total_calls == 0 {
        println!();
        println!("(empty corpus — no real calls to measure. If this is the");
        println!(" first time you're running it, agent adoption is likely the");
        println!(" bottleneck, not response shape.)");
    }
    let _ = Path::new(&path);
    Ok(())
}
