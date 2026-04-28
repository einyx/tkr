use serde_json::json;
use std::cell::RefCell;
use tkr_agent::provider::{ContentBlock, Message, Provider, ProviderResponse, StopReason};
use tkr_agent::{Manifest, ProcessTool, ToolRegistry};
use tkr_sandbox::SandboxPolicy;

struct ScriptedProvider { script: RefCell<Vec<ProviderResponse>> }
impl Provider for ScriptedProvider {
    fn send(&self, _: Option<&str>, _: &[Message], _: &[serde_json::Value], _: u32)
        -> anyhow::Result<ProviderResponse>
    {
        let mut s = self.script.borrow_mut();
        if s.is_empty() { Err(anyhow::anyhow!("script exhausted")) } else { Ok(s.remove(0)) }
    }
}

#[test]
fn agent_loop_runs_sandboxed_tool() {
    let policy = SandboxPolicy::builder().allow_read("/").build();
    let process_tool = ProcessTool::new(
        "say", "echoes a message", "/bin/echo",
        vec!["{msg}".into()],
        policy,
        json!({"type":"object","properties":{"msg":{"type":"string"}},"required":["msg"]}),
    );
    let provider = ScriptedProvider {
        script: RefCell::new(vec![
            ProviderResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "say".into(),
                    input: json!({"msg":"sandboxed"}),
                }],
                stop_reason: StopReason::ToolUse,
                input_tokens: 1, output_tokens: 1,
            },
            ProviderResponse {
                content: vec![ContentBlock::Text { text: "fin".into() }],
                stop_reason: StopReason::EndTurn,
                input_tokens: 1, output_tokens: 1,
            },
        ]),
    };
    let manifest = Manifest::parse(r#"
name = "t"
task = "test"
mode = "auto"
max_steps = 4

[model]
provider = "anthropic"
name = "x"
"#).unwrap();
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(process_tool));
    let outcome = tkr_agent::run(&manifest, &provider, &mut tools, None).unwrap();
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "fin");
    assert!(outcome.raw_bytes_total > 0);
}
