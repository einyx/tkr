use crate::tool::{Tool, ToolResult};
use anyhow::Result;
use serde_json::Value;
use tkr_sandbox::{run_sandboxed, SandboxPolicy};

pub struct ProcessTool {
    name: String,
    description: String,
    command: String,
    arg_template: Vec<ArgSlot>,
    policy: SandboxPolicy,
    input_schema: Value,
}

#[derive(Clone)]
enum ArgSlot {
    Literal(String),
    Named(String),
}

impl ProcessTool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        command: impl Into<String>,
        arg_template: Vec<String>,
        policy: SandboxPolicy,
        input_schema: Value,
    ) -> Self {
        let slots = arg_template
            .into_iter()
            .map(|s| {
                if let Some(name) = s.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
                    ArgSlot::Named(name.to_string())
                } else {
                    ArgSlot::Literal(s)
                }
            })
            .collect();
        Self {
            name: name.into(),
            description: description.into(),
            command: command.into(),
            arg_template: slots,
            policy,
            input_schema,
        }
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    fn render_args(&self, input: &Value) -> Result<Vec<String>> {
        let mut out = Vec::with_capacity(self.arg_template.len());
        for slot in &self.arg_template {
            match slot {
                ArgSlot::Literal(s) => out.push(s.clone()),
                ArgSlot::Named(k) => {
                    let v = input
                        .get(k)
                        .ok_or_else(|| anyhow::anyhow!("missing input field '{k}'"))?;
                    let s = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    out.push(s);
                }
            }
        }
        Ok(out)
    }
}

impl Tool for ProcessTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }
    fn run(&mut self, input: &Value) -> Result<ToolResult> {
        let args = self.render_args(input)?;
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run_sandboxed(&self.command, &arg_refs, &self.policy)
            .map_err(|e| anyhow::anyhow!("sandbox: {e}"))?;
        let mut content = String::from_utf8_lossy(&out.stdout).into_owned();
        if !out.stderr.is_empty() {
            content.push_str(&String::from_utf8_lossy(&out.stderr));
        }
        let raw = content.len();
        Ok(ToolResult {
            content,
            raw_bytes: raw,
            filtered_bytes: raw,
            exit: out.exit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_literal_and_named_args() {
        // Allow reading whole filesystem so the binary's dyld deps are reachable.
        let mut t = ProcessTool::new(
            "say",
            "echoes",
            "/bin/echo",
            vec!["--".into(), "{msg}".into()],
            SandboxPolicy::builder().allow_read("/").build(),
            json!({"type":"object","properties":{"msg":{"type":"string"}},"required":["msg"]}),
        );
        let r = t.run(&json!({"msg":"hi"})).unwrap();
        assert_eq!(r.exit, 0, "stderr-included content was: {}", r.content);
        assert!(r.content.contains("hi"));
    }

    #[test]
    fn missing_named_field_errors() {
        let mut t = ProcessTool::new(
            "say",
            "",
            "/bin/echo",
            vec!["{msg}".into()],
            SandboxPolicy::builder().allow_read("/").build(),
            json!({}),
        );
        assert!(t.run(&json!({})).is_err());
    }
}
