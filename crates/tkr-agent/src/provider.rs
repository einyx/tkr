use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    User { content: Vec<ContentBlock> },
    Assistant { content: Vec<ContentBlock> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other(String),
}

#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

pub trait Provider: Send {
    fn send(
        &self,
        system: Option<&str>,
        messages: &[Message],
        tools: &[serde_json::Value],
        max_tokens: u32,
    ) -> Result<ProviderResponse>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrips_through_json() {
        let m = Message::User {
            content: vec![ContentBlock::Text { text: "hi".into() }],
        };
        let s = serde_json::to_string(&m).unwrap();
        let parsed: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(m, parsed);
    }

    #[test]
    fn tool_use_block_roundtrips() {
        let b = ContentBlock::ToolUse {
            id: "tu_1".into(),
            name: "echo".into(),
            input: serde_json::json!({ "text": "hi" }),
        };
        let s = serde_json::to_string(&b).unwrap();
        let parsed: ContentBlock = serde_json::from_str(&s).unwrap();
        assert_eq!(b, parsed);
    }
}
