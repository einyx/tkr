use crate::tool::{Tool, ToolResult};
use anyhow::Result;
use serde::Deserialize;

#[derive(Deserialize)]
struct EchoInput {
    text: String,
    #[serde(default)]
    repeat: u32,
}

pub struct EchoTool;

impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" },
                "repeat": { "type": "integer", "default": 1 }
            },
            "required": ["text"]
        })
    }

    fn run(&mut self, input: &serde_json::Value) -> Result<ToolResult> {
        let parsed: EchoInput = serde_json::from_value(input.clone())?;
        let n = parsed.repeat.max(1);
        let mut out = String::new();
        for _ in 0..n {
            out.push_str(&parsed.text);
            out.push('\n');
        }
        let bytes = out.len();
        Ok(ToolResult {
            content: out,
            raw_bytes: bytes,
            filtered_bytes: bytes,
            exit: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn echoes_once_by_default() {
        let mut t = EchoTool;
        let r = t.run(&json!({ "text": "hi" })).unwrap();
        assert_eq!(r.content, "hi\n");
        assert_eq!(r.exit, 0);
    }

    #[test]
    fn echoes_n_times() {
        let mut t = EchoTool;
        let r = t.run(&json!({ "text": "x", "repeat": 3 })).unwrap();
        assert_eq!(r.content, "x\nx\nx\n");
        assert_eq!(r.raw_bytes, 6);
    }

    #[test]
    fn rejects_missing_text() {
        let mut t = EchoTool;
        assert!(t.run(&json!({})).is_err());
    }
}
