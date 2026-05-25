/// emit-fixtures — writes 3 plausible mock RunRecord files to ~/.tkr/runs/
/// for dashboard development and smoke testing.
///
/// Usage: cargo run --example emit-fixtures
use anyhow::Result;
use chrono::{Duration, Utc};
use tkr::run_record::{persist, ReceiptRecord, RunRecord};

#[allow(clippy::too_many_arguments)]
fn make_fixture(
    id: &str,
    agent: &str,
    model: &str,
    status: &str,
    steps: u32,
    input_tokens: u32,
    output_tokens: u32,
    raw_bytes: usize,
    filtered_bytes: usize,
    duration_ms: u64,
    minutes_ago: i64,
) -> RunRecord {
    let started_at = Utc::now() - Duration::minutes(minutes_ago);
    let cost_cents = tkr::run_record::estimate_cost_cents(model, input_tokens, output_tokens);
    let cost_saved_cents =
        tkr::run_record::estimate_savings_cents(model, raw_bytes, filtered_bytes);

    RunRecord {
        id: id.to_string(),
        agent: agent.to_string(),
        status: status.to_string(),
        started_at: started_at.to_rfc3339(),
        duration_ms,
        hostname: hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "localhost".to_string()),
        receipt: ReceiptRecord {
            agent: agent.to_string(),
            steps,
            input_tokens,
            output_tokens,
            raw_bytes,
            filtered_bytes,
        },
        messages: vec![],
        manifest_toml: format!(
            r#"name = "{agent}"
task = "example task"

[model]
provider = "anthropic"
name = "{model}"
"#
        ),
        cost_cents,
        cost_saved_cents,
    }
}

fn main() -> Result<()> {
    let fixtures = vec![
        make_fixture(
            "fix-a1b2c3d4",
            "code-reviewer",
            "claude-sonnet-4-6",
            "ok",
            3,
            12_500,
            4_200,
            85_000,
            21_000,
            4_312,
            45,
        ),
        make_fixture(
            "fix-e5f6a7b8",
            "doc-writer",
            "claude-haiku-4-5",
            "ok",
            1,
            3_100,
            980,
            0,
            0,
            1_820,
            20,
        ),
        make_fixture(
            "fix-c9d0e1f2",
            "test-generator",
            "claude-opus-4-7",
            "error",
            2,
            8_000,
            2_100,
            40_000,
            12_000,
            9_041,
            5,
        ),
    ];

    for record in &fixtures {
        let path = persist(record)?;
        println!("wrote: {}", path.display());
    }

    println!("\n3 fixture files written to ~/.tkr/runs/");
    Ok(())
}
