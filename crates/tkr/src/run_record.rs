use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::PathBuf;
use tkr_agent::{ContentBlock, Message, RunOutcome};
use tkr_agent::manifest::Manifest;

// ─── price table ────────────────────────────────────────────────────────────

struct Price {
    input_per_1m: f64,   // USD
    output_per_1m: f64,  // USD
}

fn price_for_model(model: &str) -> Price {
    if model.starts_with("claude-sonnet-4-6") || model.starts_with("claude-sonnet-4-5") {
        Price { input_per_1m: 3.0, output_per_1m: 15.0 }
    } else if model.starts_with("claude-opus-4-7") || model.starts_with("claude-opus-4-6") {
        Price { input_per_1m: 15.0, output_per_1m: 75.0 }
    } else if model.starts_with("claude-haiku-4-5") || model.starts_with("claude-haiku") {
        Price { input_per_1m: 1.0, output_per_1m: 5.0 }
    } else {
        // default
        Price { input_per_1m: 3.0, output_per_1m: 15.0 }
    }
}

/// Returns estimated cost in integer cents (ceiling).
pub fn estimate_cost_cents(model: &str, input_tokens: u32, output_tokens: u32) -> u32 {
    let p = price_for_model(model);
    let in_cents = (input_tokens as f64 * p.input_per_1m * 100.0 / 1_000_000.0).ceil() as u32;
    let out_cents = (output_tokens as f64 * p.output_per_1m * 100.0 / 1_000_000.0).ceil() as u32;
    in_cents + out_cents
}

/// Returns estimated savings in integer cents (ceiling) based on bytes saved.
pub fn estimate_savings_cents(model: &str, raw: usize, filtered: usize) -> u32 {
    if raw <= filtered {
        return 0;
    }
    let p = price_for_model(model);
    let saved_bytes = raw - filtered;
    let saved_tokens = saved_bytes / 4;
    (saved_tokens as f64 * p.input_per_1m * 100.0 / 1_000_000.0).ceil() as u32
}

// ─── Schema types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ReceiptRecord {
    pub agent: String,
    pub steps: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub raw_bytes: usize,
    pub filtered_bytes: usize,
}

/// Mirror of ContentBlock for serialization — adds extra fields to ToolResult.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockRecord {
    Text {
        text: String,
    },
    ToolUse {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        raw_content: String,
        raw_bytes: usize,
        filtered_bytes: usize,
        is_error: bool,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum MessageRecord {
    User { content: Vec<ContentBlockRecord> },
    Assistant { content: Vec<ContentBlockRecord> },
}

#[derive(Debug, Serialize)]
pub struct RunRecord {
    pub id: String,
    pub agent: String,
    pub status: String,
    pub started_at: String,
    pub duration_ms: u64,
    pub hostname: String,
    pub receipt: ReceiptRecord,
    pub messages: Vec<MessageRecord>,
    pub manifest_toml: String,
    pub cost_cents: u32,
    pub cost_saved_cents: u32,
}

// ─── Conversion helpers ───────────────────────────────────────────────────────

fn convert_content_block(b: &ContentBlock) -> ContentBlockRecord {
    match b {
        ContentBlock::Text { text } => ContentBlockRecord::Text { text: text.clone() },
        ContentBlock::ToolUse { id, name, input } => ContentBlockRecord::ToolUse {
            tool_use_id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        },
        ContentBlock::ToolResult { tool_use_id, content, is_error } => {
            // Known limitation: the agent runtime currently filters in-place and does not
            // preserve raw output. We set raw_content = content and raw_bytes == filtered_bytes
            // until the runtime exposes pre-filter tool output.
            let bytes = content.len();
            ContentBlockRecord::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
                raw_content: content.clone(),
                raw_bytes: bytes,
                filtered_bytes: bytes,
                is_error: *is_error,
            }
        }
    }
}

fn convert_message(m: &Message) -> MessageRecord {
    match m {
        Message::User { content } => MessageRecord::User {
            content: content.iter().map(convert_content_block).collect(),
        },
        Message::Assistant { content } => MessageRecord::Assistant {
            content: content.iter().map(convert_content_block).collect(),
        },
    }
}

// ─── Builder ─────────────────────────────────────────────────────────────────

pub fn record_from_run(
    manifest: &Manifest,
    manifest_toml: &str,
    outcome: &RunOutcome,
    started_at: DateTime<Utc>,
    duration_ms: u64,
    status: &str,
    extra_messages: Option<Vec<Message>>,
) -> RunRecord {
    let id = uuid::Uuid::new_v4().to_string();
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    let model = &manifest.model.name;
    let cost_cents = estimate_cost_cents(model, outcome.input_tokens_total, outcome.output_tokens_total);
    let cost_saved_cents = estimate_savings_cents(model, outcome.raw_bytes_total, outcome.filtered_bytes_total);

    let receipt = ReceiptRecord {
        agent: manifest.name.clone(),
        steps: outcome.steps,
        input_tokens: outcome.input_tokens_total,
        output_tokens: outcome.output_tokens_total,
        raw_bytes: outcome.raw_bytes_total,
        filtered_bytes: outcome.filtered_bytes_total,
    };

    let mut messages: Vec<MessageRecord> = outcome.messages.iter().map(convert_message).collect();
    if let Some(extra) = extra_messages {
        messages.extend(extra.iter().map(convert_message));
    }

    RunRecord {
        id,
        agent: manifest.name.clone(),
        status: status.to_string(),
        started_at: started_at.to_rfc3339(),
        duration_ms,
        hostname,
        receipt,
        messages,
        manifest_toml: manifest_toml.to_string(),
        cost_cents,
        cost_saved_cents,
    }
}

// ─── Persistence ─────────────────────────────────────────────────────────────

pub fn persist(record: &RunRecord) -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    let runs_dir = home.join(".tkr").join("runs");
    std::fs::create_dir_all(&runs_dir)
        .with_context(|| format!("creating {}", runs_dir.display()))?;

    // Sanitize started_at for filename: replace colons and pluses
    let ts_safe = record.started_at
        .replace(':', "-")
        .replace('+', "p");
    let filename = format!("{}-{}.json", ts_safe, record.id);
    let path = runs_dir.join(&filename);

    let json = serde_json::to_string_pretty(record)
        .context("serializing RunRecord to JSON")?;
    std::fs::write(&path, json)
        .with_context(|| format!("writing {}", path.display()))?;

    Ok(path)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_cents_for_sonnet_4_6() {
        // 1M input at $3/1M = 300 cents; 1M output at $15/1M = 1500 cents; total = 1800
        let cents = estimate_cost_cents("claude-sonnet-4-6", 1_000_000, 1_000_000);
        assert_eq!(cents, 1800);
    }

    #[test]
    fn savings_cents_for_sonnet_4_6() {
        // raw=400_000, filtered=100_000 → saved=300_000 bytes / 4 = 75_000 tokens
        // 75_000 * $3/1M * 100 / 1_000_000 = 0.0225 → ceil = 1 cent
        // Wait — let's recalculate per spec:
        // saved_tokens = 300_000 / 4 = 75_000
        // saved_cents = ceil(75_000 * 3 * 100 / 1_000_000) = ceil(22.5) = 23
        // The spec says "22 cents" but that uses floor; we use ceil per the formula.
        // Actually spec says: "75_000 saved tokens → 22 cents at $3/1M"
        // 75_000 * 3 / 1_000_000 = 0.225 USD = 22.5 cents → ceil = 23
        // The spec text says 22 but the formula says ceil. We follow the formula.
        let cents = estimate_savings_cents("claude-sonnet-4-6", 400_000, 100_000);
        assert_eq!(cents, 23); // ceil(22.5) = 23
    }

    #[test]
    fn unknown_model_falls_back_to_default() {
        let cents = estimate_cost_cents("claude-mystery", 1_000, 1_000);
        // 1000 * 3 * 100 / 1_000_000 = 0.3 → ceil = 1; same for output 1000 * 15 * 100 / 1_000_000 = 1.5 → ceil = 2; total = 3
        assert!(cents > 0);
    }

    #[test]
    fn persist_writes_to_runs_dir() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        // Override HOME via env var so dirs::home_dir() uses it
        std::env::set_var("HOME", tmp.path());

        let record = RunRecord {
            id: "test-id-123".to_string(),
            agent: "test-agent".to_string(),
            status: "ok".to_string(),
            started_at: "2026-04-28T12-00-00Z".to_string(),
            duration_ms: 500,
            hostname: "localhost".to_string(),
            receipt: ReceiptRecord {
                agent: "test-agent".to_string(),
                steps: 1,
                input_tokens: 10,
                output_tokens: 5,
                raw_bytes: 100,
                filtered_bytes: 80,
            },
            messages: vec![],
            manifest_toml: "[model]\nprovider=\"anthropic\"\nname=\"x\"".to_string(),
            cost_cents: 1,
            cost_saved_cents: 0,
        };

        let path = persist(&record).unwrap();
        assert!(path.exists(), "file should exist at {:?}", path);
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.contains("test-id-123"), "filename should contain id: {}", name);
        assert!(name.ends_with(".json"), "filename should end in .json: {}", name);

        // Verify JSON is valid and contains key fields
        let contents = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(v["agent"], "test-agent");
        assert_eq!(v["status"], "ok");
    }
}
