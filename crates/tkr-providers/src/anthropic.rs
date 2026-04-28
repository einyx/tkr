use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::json;
use tkr_agent::provider::{ContentBlock, Message, Provider, ProviderResponse, StopReason};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    api_key: String,
    model: String,
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
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
        let mut body = json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "messages": messages,
        });
        if let Some(s) = system {
            body["system"] = json!(s);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        body
    }

    pub(crate) fn parse_response(raw: &str) -> Result<ProviderResponse> {
        let v: ApiResponse = serde_json::from_str(raw)?;
        let stop_reason = match v.stop_reason.as_deref() {
            Some("end_turn") => StopReason::EndTurn,
            Some("tool_use") => StopReason::ToolUse,
            Some("max_tokens") => StopReason::MaxTokens,
            Some(s) => StopReason::Other(s.to_string()),
            None => StopReason::Other("missing".into()),
        };
        Ok(ProviderResponse {
            content: v.content,
            stop_reason,
            input_tokens: v.usage.input_tokens,
            output_tokens: v.usage.output_tokens,
        })
    }
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    usage: Usage,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u32,
    output_tokens: u32,
}

impl Provider for AnthropicProvider {
    fn send(
        &self,
        system: Option<&str>,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: u32,
    ) -> Result<ProviderResponse> {
        let body = self.build_request(system, messages, tools, max_tokens);
        let url = format!("{}/v1/messages", self.base_url);
        let resp = ureq::post(&url)
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", API_VERSION)
            .set("content-type", "application/json")
            .send_json(body);
        let resp = match resp {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                return Err(anyhow!("anthropic api {}: {}", code, body));
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
    fn build_request_includes_system_and_tools() {
        let p = AnthropicProvider::new("k", "claude-sonnet-4-6");
        let msgs = vec![Message::User { content: vec![ContentBlock::Text { text: "hi".into() }] }];
        let tools = vec![json!({ "name": "echo", "input_schema": {} })];
        let body = p.build_request(Some("be brief"), &msgs, &tools, 256);
        assert_eq!(body["model"], "claude-sonnet-4-6");
        assert_eq!(body["max_tokens"], 256);
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["tools"][0]["name"], "echo");
    }

    #[test]
    fn build_request_omits_system_when_none() {
        let p = AnthropicProvider::new("k", "claude-sonnet-4-6");
        let body = p.build_request(None, &[], &[], 16);
        assert!(body.get("system").is_none());
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn parse_response_text_only() {
        let raw = r#"{
            "content": [{"type":"text","text":"hello"}],
            "stop_reason":"end_turn",
            "usage":{"input_tokens":3,"output_tokens":1}
        }"#;
        let r = AnthropicProvider::parse_response(raw).unwrap();
        assert_eq!(r.input_tokens, 3);
        assert_eq!(r.output_tokens, 1);
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        match &r.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "hello"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn parse_response_tool_use() {
        let raw = r#"{
            "content": [{"type":"tool_use","id":"tu_1","name":"echo","input":{"text":"x"}}],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":5,"output_tokens":4}
        }"#;
        let r = AnthropicProvider::parse_response(raw).unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
    }
}
