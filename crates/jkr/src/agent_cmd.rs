use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use std::io::Write;
use std::path::Path;
use std::time::Instant;
use jkr_agent::{tools::echo::EchoTool, ContentBlock, EventSink, Manifest, Message, RunReceipt, ToolRegistry};
use jkr_api::AgentEvent;
use jkr_providers::AnthropicProvider;

use jkr::run_record;

/// Writes each event as one NDJSON line, flushing per line so a reader
/// (e.g. a Docker log stream) sees events as they happen.
pub struct NdjsonSink<W: Write> {
    out: W,
}

impl<W: Write> NdjsonSink<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write> EventSink for NdjsonSink<W> {
    fn write(&mut self, event: AgentEvent) {
        let _ = writeln!(self.out, "{}", event.to_ndjson());
        let _ = self.out.flush();
    }
}

pub fn run_agent(manifest_path: &Path, stream: bool) -> Result<()> {
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

    let run_result = if stream {
        let stdout = std::io::stdout();
        let mut sink = NdjsonSink::new(stdout.lock());
        jkr_agent::run(&manifest, &provider, &mut tools, None, Some(&mut sink))
    } else {
        jkr_agent::run(&manifest, &provider, &mut tools, None, None)
    };
    let duration_ms = clock.elapsed().as_millis() as u64;

    match run_result {
        Ok(outcome) => {
            if !stream {
                println!("{}", outcome.final_text);
                println!();
                let receipt = RunReceipt::from_outcome(&manifest.name, &outcome);
                println!("{receipt}");
            }

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
                Ok(path) => {
                    if !stream {
                        println!("   record:        {}", path.display());
                    }
                }
                Err(e) => eprintln!("jkr: warning: could not persist run record: {e}"),
            }

            Ok(())
        }
        Err(err) => {
            // Build a synthetic outcome with an error message appended as an assistant block
            let error_text = format!("error: {err:#}");
            let error_outcome = jkr_agent::RunOutcome {
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
                eprintln!("jkr: warning: could not persist run record: {persist_err}");
            }

            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jkr_api::{AgentEvent, AgentEventKind};

    #[test]
    fn ndjson_sink_writes_one_line_per_event() {
        let mut buf: Vec<u8> = Vec::new();
        let mut sink = NdjsonSink::new(&mut buf);

        sink.write(AgentEvent { seq: 0, kind: AgentEventKind::Step { step: 1, max: 5 } });
        sink.write(AgentEvent { seq: 1, kind: AgentEventKind::Done { exit: 0, steps: 1 } });

        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"t\":\"step\""), "line0: {}", lines[0]);
        assert!(lines[1].contains("\"t\":\"done\""), "line1: {}", lines[1]);
    }
}
