use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::json;
use tkr_agent::provider::{ContentBlock, Message, Provider, ProviderResponse, StopReason};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

pub struct OllamaProvider {
    model: String,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            base_url: DEFAULT_BASE_URL.into(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub(crate) fn build_request(
        &self,
        system: Option<&str>,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: u32,
    ) -> serde_json::Value {
        let mut openai_messages: Vec<serde_json::Value> = Vec::new();
        if let Some(s) = system {
            openai_messages.push(json!({ "role": "system", "content": s }));
        }
        for m in messages {
            translate_message(m, &mut openai_messages);
        }

        let mut body = json!({
            "model": self.model,
            "messages": openai_messages,
            "max_tokens": max_tokens,
            "stream": false,
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools.iter().map(translate_tool).collect::<Vec<_>>());
        }
        body
    }

    pub(crate) fn parse_response(raw: &str) -> Result<ProviderResponse> {
        let v: ApiResponse = serde_json::from_str(raw)?;
        let choice = v
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("ollama response missing choices"))?;

        let mut content: Vec<ContentBlock> = Vec::new();
        if let Some(text) = choice.message.content.filter(|s| !s.is_empty()) {
            content.push(ContentBlock::Text { text });
        }
        for tc in choice.message.tool_calls.unwrap_or_default() {
            let input = repair_tool_args(&tc.function.arguments)?;
            content.push(ContentBlock::ToolUse {
                id: tc.id,
                name: tc.function.name,
                input,
            });
        }

        let stop_reason = match choice.finish_reason.as_deref() {
            Some("stop") => StopReason::EndTurn,
            Some("tool_calls") => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            Some(s) => StopReason::Other(s.to_string()),
            None if content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. })) =>
            {
                StopReason::ToolUse
            }
            None => StopReason::EndTurn,
        };

        Ok(ProviderResponse {
            content,
            stop_reason,
            input_tokens: v.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0),
            output_tokens: v.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
        })
    }
}

/// Convert one Anthropic-shaped `Message` into one or more OpenAI-shaped messages.
///
/// Anthropic packs multiple tool_results into a single user message; OpenAI requires
/// one `role:"tool"` message per tool_call_id. So a single User message can fan out.
fn translate_message(m: &Message, out: &mut Vec<serde_json::Value>) {
    match m {
        Message::User { content } => {
            let mut text_parts: Vec<String> = Vec::new();
            for block in content {
                match block {
                    ContentBlock::Text { text } => text_parts.push(text.clone()),
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let body = if *is_error {
                            format!("ERROR: {content}")
                        } else {
                            content.clone()
                        };
                        out.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id,
                            "content": body,
                        }));
                    }
                    ContentBlock::ToolUse { .. } => {}
                }
            }
            if !text_parts.is_empty() {
                out.push(json!({ "role": "user", "content": text_parts.join("\n") }));
            }
        }
        Message::Assistant { content } => {
            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_calls: Vec<serde_json::Value> = Vec::new();
            for block in content {
                match block {
                    ContentBlock::Text { text } => text_parts.push(text.clone()),
                    ContentBlock::ToolUse { id, name, input } => {
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".into()),
                            },
                        }));
                    }
                    ContentBlock::ToolResult { .. } => {}
                }
            }
            let mut msg = json!({ "role": "assistant" });
            if !text_parts.is_empty() {
                msg["content"] = json!(text_parts.join("\n"));
            } else {
                msg["content"] = serde_json::Value::Null;
            }
            if !tool_calls.is_empty() {
                msg["tool_calls"] = json!(tool_calls);
            }
            out.push(msg);
        }
    }
}

/// Anthropic tool: `{ "name", "description", "input_schema" }`
/// OpenAI tool:    `{ "type":"function", "function": { "name", "description", "parameters" } }`
fn translate_tool(t: &serde_json::Value) -> serde_json::Value {
    let name = t.get("name").cloned().unwrap_or(json!(""));
    let description = t.get("description").cloned().unwrap_or(json!(""));
    let parameters = t
        .get("input_schema")
        .or_else(|| t.get("parameters"))
        .cloned()
        .unwrap_or(json!({ "type": "object", "properties": {} }));
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        },
    })
}

/// Local models occasionally emit malformed JSON in `tool_calls[].function.arguments`
/// (trailing commas, unquoted keys, single quotes, embedded prose).
///
/// Strategy: strict JSON → json5 (handles trailing commas, unquoted keys, single
/// quotes) → empty object. The agent loop sees a well-formed tool call either way;
/// a tool that needed an arg will fail at execution and feed the error back so the
/// model self-corrects on the next turn.
fn repair_tool_args(raw: &str) -> Result<serde_json::Value> {
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        return Ok(v);
    }
    if let Ok(v) = json5::from_str::<serde_json::Value>(raw) {
        return Ok(v);
    }
    Ok(json!({}))
}

#[derive(Deserialize)]
struct ApiResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Deserialize)]
struct ToolCall {
    id: String,
    function: ToolCallFunction,
}

#[derive(Deserialize)]
struct ToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

impl Provider for OllamaProvider {
    fn send(
        &self,
        system: Option<&str>,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: u32,
    ) -> Result<ProviderResponse> {
        let body = self.build_request(system, messages, tools, max_tokens);
        let url = format!("{}/v1/chat/completions", self.base_url);
        let resp = ureq::post(&url)
            .set("content-type", "application/json")
            .send_json(body);
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                return Err(anyhow!("ollama api {code}: {body}"));
            }
            Err(e) => return Err(anyhow!(e)),
        };
        let raw = resp.into_string()?;
        Self::parse_response(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_translates_system_and_user_text() {
        let p = OllamaProvider::new("qwen2.5-coder:14b");
        let msgs = vec![Message::User {
            content: vec![ContentBlock::Text { text: "hi".into() }],
        }];
        let body = p.build_request(Some("be brief"), &msgs, &[], 256);
        assert_eq!(body["model"], "qwen2.5-coder:14b");
        assert_eq!(body["max_tokens"], 256);
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be brief");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "hi");
    }

    #[test]
    fn build_request_translates_tool_schema() {
        let p = OllamaProvider::new("llama3.1");
        let tools = vec![json!({
            "name": "echo",
            "description": "say it back",
            "input_schema": { "type": "object", "properties": { "text": { "type": "string" } } }
        })];
        let body = p.build_request(None, &[], &tools, 16);
        let t = &body["tools"][0];
        assert_eq!(t["type"], "function");
        assert_eq!(t["function"]["name"], "echo");
        assert_eq!(t["function"]["description"], "say it back");
        assert_eq!(t["function"]["parameters"]["properties"]["text"]["type"], "string");
    }

    #[test]
    fn assistant_tool_use_becomes_tool_calls() {
        let p = OllamaProvider::new("m");
        let msgs = vec![Message::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "echo".into(),
                input: json!({ "text": "hi" }),
            }],
        }];
        let body = p.build_request(None, &msgs, &[], 16);
        let m = &body["messages"][0];
        assert_eq!(m["role"], "assistant");
        assert_eq!(m["tool_calls"][0]["id"], "tu_1");
        assert_eq!(m["tool_calls"][0]["function"]["name"], "echo");
        let args: serde_json::Value =
            serde_json::from_str(m["tool_calls"][0]["function"]["arguments"].as_str().unwrap())
                .unwrap();
        assert_eq!(args["text"], "hi");
    }

    #[test]
    fn user_tool_result_fans_out_to_tool_messages() {
        let p = OllamaProvider::new("m");
        let msgs = vec![Message::User {
            content: vec![
                ContentBlock::ToolResult {
                    tool_use_id: "tu_1".into(),
                    content: "ok".into(),
                    is_error: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "tu_2".into(),
                    content: "boom".into(),
                    is_error: true,
                },
            ],
        }];
        let body = p.build_request(None, &msgs, &[], 16);
        let ms = body["messages"].as_array().unwrap();
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[0]["role"], "tool");
        assert_eq!(ms[0]["tool_call_id"], "tu_1");
        assert_eq!(ms[0]["content"], "ok");
        assert_eq!(ms[1]["tool_call_id"], "tu_2");
        assert!(ms[1]["content"].as_str().unwrap().starts_with("ERROR:"));
    }

    #[test]
    fn parse_response_text_only() {
        let raw = r#"{
            "choices":[{"message":{"content":"hello"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":3,"completion_tokens":1}
        }"#;
        let r = OllamaProvider::parse_response(raw).unwrap();
        assert_eq!(r.input_tokens, 3);
        assert_eq!(r.output_tokens, 1);
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        match &r.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_response_tool_call() {
        let raw = r#"{
            "choices":[{"message":{"content":null,"tool_calls":[
                {"id":"tu_1","type":"function","function":{"name":"echo","arguments":"{\"text\":\"x\"}"}}
            ]},"finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":5,"completion_tokens":4}
        }"#;
        let r = OllamaProvider::parse_response(raw).unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        match &r.content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "tu_1");
                assert_eq!(name, "echo");
                assert_eq!(input["text"], "x");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn repair_tool_args_handles_local_model_quirks() {
        // strict json
        assert_eq!(repair_tool_args(r#"{"a":1}"#).unwrap()["a"], 1);
        // empty
        assert_eq!(repair_tool_args("").unwrap(), json!({}));
        // trailing comma
        assert_eq!(repair_tool_args(r#"{"a":1,}"#).unwrap()["a"], 1);
        // single quotes
        assert_eq!(repair_tool_args(r#"{'a':'hi'}"#).unwrap()["a"], "hi");
        // unquoted keys
        assert_eq!(repair_tool_args(r#"{a:1}"#).unwrap()["a"], 1);
        // total garbage → empty object, never errors
        assert_eq!(repair_tool_args("not json at all").unwrap(), json!({}));
    }

    #[test]
    fn parse_response_text_and_tool_call() {
        let raw = r#"{
            "choices":[{"message":{"content":"thinking...","tool_calls":[
                {"id":"tu_9","type":"function","function":{"name":"ls","arguments":"{}"}}
            ]},"finish_reason":"tool_calls"}],
            "usage":{"prompt_tokens":1,"completion_tokens":1}
        }"#;
        let r = OllamaProvider::parse_response(raw).unwrap();
        assert_eq!(r.content.len(), 2);
        assert!(matches!(r.content[0], ContentBlock::Text { .. }));
        assert!(matches!(r.content[1], ContentBlock::ToolUse { .. }));
    }
}
