use crate::manifest::Manifest;
use crate::provider::{ContentBlock, Message, Provider, StopReason};
use crate::tool::{ToolRegistry, ToolResult};
use anyhow::{anyhow, Result};
use jkr_api::{FilterResult, LegacyPlugin as Plugin};
use jkr_filter::FilterPlugin;

#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub final_text: String,
    pub steps: u32,
    pub input_tokens_total: u32,
    pub output_tokens_total: u32,
    pub raw_bytes_total: usize,
    pub filtered_bytes_total: usize,
    pub messages: Vec<Message>,
}

pub fn run(
    manifest: &Manifest,
    provider: &dyn Provider,
    tools: &mut ToolRegistry,
    filter: Option<&mut FilterPlugin>,
) -> Result<RunOutcome> {
    let schemas = tools.schemas();

    let mut messages: Vec<Message> = vec![Message::User {
        content: vec![ContentBlock::Text {
            text: manifest.task.clone(),
        }],
    }];

    let mut steps = 0u32;
    let mut input_tokens_total = 0u32;
    let mut output_tokens_total = 0u32;
    let mut raw_bytes_total = 0usize;
    let mut filtered_bytes_total = 0usize;
    let mut final_text = String::new();
    let mut filter = filter;

    while steps < manifest.max_steps {
        steps += 1;
        let resp = provider.send(manifest.system.as_deref(), &messages, &schemas, 1024)?;
        input_tokens_total += resp.input_tokens;
        output_tokens_total += resp.output_tokens;

        messages.push(Message::Assistant {
            content: resp.content.clone(),
        });

        match resp.stop_reason {
            StopReason::EndTurn | StopReason::MaxTokens => {
                final_text = collect_text(&resp.content);
                return Ok(RunOutcome {
                    final_text,
                    steps,
                    input_tokens_total,
                    output_tokens_total,
                    raw_bytes_total,
                    filtered_bytes_total,
                    messages,
                });
            }
            StopReason::ToolUse => {
                let raw_results = run_tool_calls(&resp.content, tools)?;
                let mut blocks = Vec::with_capacity(raw_results.len());
                for (id, mut tr, tool_name) in raw_results {
                    raw_bytes_total += tr.raw_bytes;
                    if let Some(f) = filter.as_deref_mut() {
                        tr.content = apply_filter(f, &tool_name, &tr.content);
                        tr.filtered_bytes = tr.content.len();
                    }
                    filtered_bytes_total += tr.filtered_bytes;
                    blocks.push(ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: tr.content,
                        is_error: tr.exit != 0,
                    });
                }
                messages.push(Message::User { content: blocks });
            }
            StopReason::Other(s) => {
                return Err(anyhow!("unexpected stop reason: {s}"));
            }
        }
    }

    Ok(RunOutcome {
        final_text,
        steps,
        input_tokens_total,
        output_tokens_total,
        raw_bytes_total,
        filtered_bytes_total,
        messages,
    })
}

fn collect_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_tool_calls(
    blocks: &[ContentBlock],
    tools: &mut ToolRegistry,
) -> Result<Vec<(String, ToolResult, String)>> {
    let mut out = Vec::new();
    for b in blocks {
        if let ContentBlock::ToolUse { id, name, input } = b {
            let tool = tools
                .get_mut(name)
                .ok_or_else(|| anyhow!("unknown tool: {name}"))?;
            let res = tool.run(input)?;
            out.push((id.clone(), res, name.clone()));
        }
    }
    Ok(out)
}

fn apply_filter(plugin: &mut FilterPlugin, tool_name: &str, content: &str) -> String {
    let mut out = String::new();
    for (idx, line) in content.lines().enumerate() {
        match plugin.filter(line, tool_name, "", idx as u64) {
            FilterResult::Pass => {
                out.push_str(line);
                out.push('\n');
            }
            FilterResult::Suppress | FilterResult::SuppressWithNote(_) => {}
            FilterResult::Replace(s) => {
                out.push_str(&s);
                out.push('\n');
            }
            FilterResult::Annotate(_) => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    let summary = plugin.flush();
    if !summary.is_empty() {
        out.push_str(&summary);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{AgentMode, Manifest, ModelDecl};
    use crate::provider::ProviderResponse;
    use crate::tool::Tool;
    use serde_json::json;
    use std::cell::RefCell;

    struct ScriptedProvider {
        script: RefCell<Vec<ProviderResponse>>,
    }
    impl Provider for ScriptedProvider {
        fn send(
            &self,
            _system: Option<&str>,
            _messages: &[Message],
            _tools: &[serde_json::Value],
            _max_tokens: u32,
        ) -> Result<ProviderResponse> {
            let mut s = self.script.borrow_mut();
            if s.is_empty() {
                Err(anyhow!("script exhausted"))
            } else {
                Ok(s.remove(0))
            }
        }
    }

    struct LoudEcho;
    impl Tool for LoudEcho {
        fn name(&self) -> &str {
            "echo"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({})
        }
        fn run(&mut self, _input: &serde_json::Value) -> Result<ToolResult> {
            let s = "EE\n".to_string();
            Ok(ToolResult {
                content: s.clone(),
                raw_bytes: s.len(),
                filtered_bytes: s.len(),
                exit: 0,
            })
        }
    }

    fn manifest() -> Manifest {
        let _ = AgentMode::Approve;
        Manifest {
            name: "t".into(),
            model: ModelDecl {
                provider: "anthropic".into(),
                name: "x".into(),
            },
            system: None,
            task: "say hi".into(),
            tools: vec![],
            mode: AgentMode::Auto,
            max_steps: 5,
            sandbox: None,
            secrets: Default::default(),
        }
    }

    #[test]
    fn loop_terminates_on_end_turn() {
        let provider = ScriptedProvider {
            script: RefCell::new(vec![ProviderResponse {
                content: vec![ContentBlock::Text {
                    text: "hi back".into(),
                }],
                stop_reason: StopReason::EndTurn,
                input_tokens: 1,
                output_tokens: 1,
            }]),
        };
        let mut tools = ToolRegistry::new();
        let out = run(&manifest(), &provider, &mut tools, None).unwrap();
        assert_eq!(out.final_text, "hi back");
        assert_eq!(out.steps, 1);
    }

    #[test]
    fn loop_executes_tool_call_then_finishes() {
        let provider = ScriptedProvider {
            script: RefCell::new(vec![
                ProviderResponse {
                    content: vec![ContentBlock::ToolUse {
                        id: "tu_1".into(),
                        name: "echo".into(),
                        input: json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    input_tokens: 2,
                    output_tokens: 2,
                },
                ProviderResponse {
                    content: vec![ContentBlock::Text {
                        text: "done".into(),
                    }],
                    stop_reason: StopReason::EndTurn,
                    input_tokens: 1,
                    output_tokens: 1,
                },
            ]),
        };
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(LoudEcho));
        let out = run(&manifest(), &provider, &mut tools, None).unwrap();
        assert_eq!(out.steps, 2);
        assert_eq!(out.final_text, "done");
        assert_eq!(out.raw_bytes_total, 3);
    }

    #[test]
    fn loop_caps_at_max_steps() {
        let mut script = Vec::new();
        for _ in 0..10 {
            script.push(ProviderResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tu".into(),
                    name: "echo".into(),
                    input: json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                input_tokens: 0,
                output_tokens: 0,
            });
        }
        let provider = ScriptedProvider {
            script: RefCell::new(script),
        };
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(LoudEcho));
        let mut m = manifest();
        m.max_steps = 3;
        let out = run(&m, &provider, &mut tools, None).unwrap();
        assert_eq!(out.steps, 3);
    }

    #[test]
    fn filter_compresses_tool_output() {
        use jkr_filter::FilterPlugin;
        // Rule schema confirmed from jkr-filter/src/rules.rs:
        // rules use `type = "suppress_regex"` with `pattern` field.
        // (Not `match`/`action = "suppress"` as suggested in the plan.)
        let filter_toml = r#"
command = "echo"
[[rules]]
type = "suppress_regex"
pattern = "^DROP "
"#;
        let mut filter = FilterPlugin::from_toml(filter_toml).unwrap();

        struct NoisyEcho;
        impl Tool for NoisyEcho {
            fn name(&self) -> &str {
                "echo"
            }
            fn input_schema(&self) -> serde_json::Value {
                json!({})
            }
            fn run(&mut self, _input: &serde_json::Value) -> Result<ToolResult> {
                let s = "KEEP one\nDROP two\nKEEP three\n".to_string();
                Ok(ToolResult {
                    content: s.clone(),
                    raw_bytes: s.len(),
                    filtered_bytes: s.len(),
                    exit: 0,
                })
            }
        }

        let provider = ScriptedProvider {
            script: RefCell::new(vec![
                ProviderResponse {
                    content: vec![ContentBlock::ToolUse {
                        id: "tu".into(),
                        name: "echo".into(),
                        input: json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    input_tokens: 0,
                    output_tokens: 0,
                },
                ProviderResponse {
                    content: vec![ContentBlock::Text { text: "ok".into() }],
                    stop_reason: StopReason::EndTurn,
                    input_tokens: 0,
                    output_tokens: 0,
                },
            ]),
        };
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(NoisyEcho));
        let out = run(&manifest(), &provider, &mut tools, Some(&mut filter)).unwrap();
        assert!(out.filtered_bytes_total < out.raw_bytes_total);
    }
}
