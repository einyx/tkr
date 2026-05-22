use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::path::Path;
use std::time::Instant;
use tkr_agent::{tools::echo::EchoTool, ContentBlock, Manifest, Message, RunReceipt, ToolRegistry};
use tkr_providers::AnthropicProvider;

use tkr::run_record;

pub fn run_agent(manifest_path: &Path) -> Result<()> {
    let manifest_toml = std::fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest = Manifest::parse(&manifest_toml)
        .with_context(|| format!("loading manifest {}", manifest_path.display()))?;

    let mut tools = ToolRegistry::new();
    for decl in &manifest.tools {
        match decl.name.as_str() {
            "echo" => tools.register(Box::new(EchoTool)),
            other => return Err(anyhow!("unknown tool '{other}' (v1 only ships 'echo')")),
        }
    }

    let provider = match manifest.model.provider.as_str() {
        "anthropic" => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| anyhow!("ANTHROPIC_API_KEY not set"))?;
            AnthropicProvider::new(key, &manifest.model.name)
        }
        other => {
            return Err(anyhow!(
                "unknown provider '{other}' (v1 only ships 'anthropic')"
            ))
        }
    };

    let started_at = Utc::now();
    let clock = Instant::now();

    let run_result = tkr_agent::run(&manifest, &provider, &mut tools, None);
    let duration_ms = clock.elapsed().as_millis() as u64;

    match run_result {
        Ok(outcome) => {
            println!("{}", outcome.final_text);
            println!();
            let receipt = RunReceipt::from_outcome(&manifest.name, &outcome);
            println!("{receipt}");

            let record = run_record::record_from_run(
                &manifest,
                &manifest_toml,
                &outcome,
                started_at,
                duration_ms,
                "ok",
                None,
            );

            match run_record::persist(&record) {
                Ok(path) => println!("   record:        {}", path.display()),
                Err(e) => eprintln!("tkr: warning: could not persist run record: {e}"),
            }

            Ok(())
        }
        Err(err) => {
            // Build a synthetic outcome with an error message appended as an assistant block
            let error_text = format!("error: {err:#}");
            let error_outcome = tkr_agent::RunOutcome {
                final_text: error_text.clone(),
                steps: 0,
                input_tokens_total: 0,
                output_tokens_total: 0,
                raw_bytes_total: 0,
                filtered_bytes_total: 0,
                messages: vec![],
            };

            // Synthetic error message for the dashboard
            let extra_messages = vec![Message::Assistant {
                content: vec![ContentBlock::Text { text: error_text }],
            }];

            let record = run_record::record_from_run(
                &manifest,
                &manifest_toml,
                &error_outcome,
                started_at,
                duration_ms,
                "error",
                Some(extra_messages),
            );

            if let Err(persist_err) = run_record::persist(&record) {
                eprintln!("tkr: warning: could not persist run record: {persist_err}");
            }

            Err(err)
        }
    }
}
