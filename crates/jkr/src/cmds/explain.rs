use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const MAX_RECORD_BYTES: u64 = 20 * 1024 * 1024;

pub fn run(file: Option<PathBuf>) -> Result<()> {
    let record_path = match file {
        Some(p) => p,
        None => latest_run_record()
            .context("no run records found; run `jkr agent run <manifest.toml>` first")?,
    };
    let meta =
        fs::metadata(&record_path).with_context(|| format!("stat {}", record_path.display()))?;
    if meta.len() > MAX_RECORD_BYTES {
        anyhow::bail!(
            "run record too large ({} bytes > {} bytes): {}",
            meta.len(),
            MAX_RECORD_BYTES,
            record_path.display()
        );
    }
    let text = fs::read_to_string(&record_path)
        .with_context(|| format!("reading {}", record_path.display()))?;
    let v: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", record_path.display()))?;

    let agent = safe(v.get("agent").and_then(|x| x.as_str()).unwrap_or("?"));
    let status = safe(v.get("status").and_then(|x| x.as_str()).unwrap_or("?"));
    let started_at = safe(v.get("started_at").and_then(|x| x.as_str()).unwrap_or("?"));
    let receipt = v.get("receipt").cloned().unwrap_or(Value::Null);
    let raw_total = receipt
        .get("raw_bytes")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let filtered_total = receipt
        .get("filtered_bytes")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let saved_total = raw_total.saturating_sub(filtered_total);

    println!("jkr explain");
    println!("  record: {}", record_path.display());
    println!("  agent: {agent}  status: {status}  started: {started_at}");
    println!(
        "  total: raw={} filtered={} saved={} (~{} tokens)",
        raw_total,
        filtered_total,
        saved_total,
        saved_total / 4
    );

    let mut rows = Vec::new();
    if let Some(messages) = v.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            if msg.get("role").and_then(|r| r.as_str()) != Some("assistant") {
                continue;
            }
            let Some(content) = msg.get("content").and_then(|c| c.as_array()) else {
                continue;
            };
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                    continue;
                }
                let tool_id = safe(
                    block
                        .get("tool_use_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("?"),
                );
                let raw = block.get("raw_bytes").and_then(|x| x.as_u64()).unwrap_or(0);
                let filtered = block
                    .get("filtered_bytes")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                rows.push((tool_id.to_string(), raw, filtered));
            }
        }
    }

    if rows.is_empty() {
        println!("\nNo tool results found in this run record.");
        return Ok(());
    }

    println!("\nPer tool result:");
    for (tool_id, raw, filtered) in rows {
        let saved = raw.saturating_sub(filtered);
        println!(
            "  {tool_id:<20} raw={raw:<7} filtered={filtered:<7} saved={saved:<7} (~{} toks)",
            saved / 4
        );
    }
    println!("\nNote: for some runs raw output is not yet persisted separately, so per-tool saved may show 0.");
    Ok(())
}

fn safe(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

fn latest_run_record() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let runs = home.join(".jkr").join("runs");
    let mut entries: Vec<PathBuf> = fs::read_dir(runs)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    entries.sort();
    entries.pop()
}
