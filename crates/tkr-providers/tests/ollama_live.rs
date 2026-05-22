//! Live smoke tests against a real Ollama daemon.
//!
//! Skipped by default. Run with:
//!   cargo test -p tkr-providers --test ollama_live -- --ignored --nocapture
//!
//! Requires:
//!   - Ollama running on http://localhost:11434
//!   - `ollama pull qwen2.5-coder:7b`

use serde_json::json;
use tkr_agent::provider::{ContentBlock, Message, Provider, StopReason};
use tkr_providers::OllamaProvider;

const MODEL: &str = "qwen2.5-coder:7b";

#[test]
#[ignore]
fn live_text_round_trip() {
    let p = OllamaProvider::new(MODEL);
    let msgs = vec![Message::User {
        content: vec![ContentBlock::Text {
            text: "Reply with exactly one word: pong".into(),
        }],
    }];
    let resp = p.send(Some("You are terse."), &msgs, &[], 32).unwrap();
    eprintln!("stop={:?} in={} out={}", resp.stop_reason, resp.input_tokens, resp.output_tokens);
    eprintln!("content={:#?}", resp.content);
    assert!(matches!(resp.stop_reason, StopReason::EndTurn | StopReason::MaxTokens));
    assert!(resp.content.iter().any(|b| matches!(b, ContentBlock::Text { .. })));
}

#[test]
#[ignore]
fn live_tool_use() {
    let p = OllamaProvider::new(MODEL);
    let tools = vec![json!({
        "name": "get_weather",
        "description": "Get the current weather for a city.",
        "input_schema": {
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" }
            },
            "required": ["city"]
        }
    })];
    let msgs = vec![Message::User {
        content: vec![ContentBlock::Text {
            text: "What's the weather in Paris? Use the tool.".into(),
        }],
    }];
    let resp = p
        .send(
            Some("You are a helpful assistant. Use tools when asked."),
            &msgs,
            &tools,
            128,
        )
        .unwrap();
    eprintln!("stop={:?}", resp.stop_reason);
    eprintln!("content={:#?}", resp.content);
    let used_tool = resp
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "get_weather"));
    assert!(used_tool, "model did not call the tool; content={:?}", resp.content);
}
