use anyhow::Result;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub content: String,
    pub raw_bytes: usize,
    pub filtered_bytes: usize,
    pub exit: i32,
}

impl ToolResult {
    pub fn is_error(&self) -> bool {
        self.exit != 0
    }
}

pub trait Tool: Send {
    fn name(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    fn run(&mut self, input: &serde_json::Value) -> Result<ToolResult>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Box<dyn Tool>> {
        self.tools.get_mut(name)
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools
            .values()
            .map(|t| {
                serde_json::json!({
                    "name": t.name(),
                    "input_schema": t.input_schema(),
                })
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Stub;
    impl Tool for Stub {
        fn name(&self) -> &str {
            "stub"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }
        fn run(&mut self, _input: &serde_json::Value) -> Result<ToolResult> {
            Ok(ToolResult {
                content: "ok".into(),
                raw_bytes: 2,
                filtered_bytes: 2,
                exit: 0,
            })
        }
    }

    #[test]
    fn registry_holds_tools() {
        let mut r = ToolRegistry::new();
        r.register(Box::new(Stub));
        assert_eq!(r.names(), vec!["stub".to_string()]);
        assert!(r.get_mut("stub").is_some());
        assert!(r.get_mut("missing").is_none());
    }

    #[test]
    fn schemas_includes_name() {
        let mut r = ToolRegistry::new();
        r.register(Box::new(Stub));
        let s = r.schemas();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0]["name"], "stub");
    }
}
