# tkr-agent Runtime MVP — Implementation Plan (Plan 1 of 6)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a working `tkr agent run <manifest.toml>` that drives an Anthropic-backed agent loop, executes typed tools, filters their output through `tkr-filter` for the model, and prints a token-savings receipt.

**Architecture:** Two new workspace crates. `tkr-agent` owns the manifest schema, the run loop, and the tool/provider traits. `tkr-providers` owns the Anthropic HTTP client. The existing `tkr-filter` crate is reused as the egress chokepoint between tool output and model context. The existing `tkr` binary gets a new `agent run` subcommand. Sandbox, vault, real infra tools, and the dashboard are all out of scope (Plans 2–6).

**Tech Stack:** Rust 2021, `tokio`-free (synchronous, like the rest of the repo), `ureq` for HTTP, `serde`+`toml` for manifests, `anyhow` for errors, `mockito` for HTTP tests, `clap` for CLI.

**Spec reference:** `docs/superpowers/specs/2026-04-28-tkr-agents-platform-design.md` §7.3, §7.4, §8.

---

## File Structure

**New crates:**
- `crates/tkr-agent/Cargo.toml` — agent runtime crate manifest
- `crates/tkr-agent/src/lib.rs` — public exports
- `crates/tkr-agent/src/manifest.rs` — TOML manifest types + parser
- `crates/tkr-agent/src/tool.rs` — `Tool` trait + `ToolResult` type + `ToolRegistry`
- `crates/tkr-agent/src/provider.rs` — `Provider` trait + message types
- `crates/tkr-agent/src/loop_.rs` — agent loop / executor
- `crates/tkr-agent/src/receipt.rs` — `RunReceipt` struct + `Display`
- `crates/tkr-agent/src/tools/echo.rs` — stub echo tool for tests + smoke runs
- `crates/tkr-agent/src/tools/mod.rs` — tools module index
- `crates/tkr-agent/tests/loop_integration.rs` — end-to-end with mock provider

- `crates/tkr-providers/Cargo.toml`
- `crates/tkr-providers/src/lib.rs`
- `crates/tkr-providers/src/anthropic.rs` — Anthropic Messages API client

**Modified:**
- `Cargo.toml` (workspace) — add new members + workspace deps
- `crates/tkr/Cargo.toml` — depend on `tkr-agent`, `tkr-providers`
- `crates/tkr/src/cli.rs` — new `Agent { Run { manifest: PathBuf } }` subcommand
- `crates/tkr/src/dispatch.rs` (or `main.rs`) — route `Agent::Run` to `tkr-agent`

**New examples:**
- `examples/hello.toml` — minimal manifest exercising the echo tool

---

## Task 1: Scaffold the two new crates

**Files:**
- Create: `crates/tkr-agent/Cargo.toml`
- Create: `crates/tkr-agent/src/lib.rs`
- Create: `crates/tkr-providers/Cargo.toml`
- Create: `crates/tkr-providers/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1.1: Add workspace members + new workspace deps**

Edit `/tmp/tkr-work/Cargo.toml`. Replace the `members = [...]` and `[workspace.dependencies]` sections:

```toml
[workspace]
resolver = "2"
members = [
    "crates/tkr-api",
    "crates/tkr-filter",
    "crates/tkr-semantic",
    "crates/tkr-analytics",
    "crates/tkr-agent",
    "crates/tkr-providers",
    "crates/tkr",
]

[workspace.dependencies]
anyhow = "1.0"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
regex = "1"
dirs = "5"
chrono = "0.4"
clap = { version = "4", features = ["derive"] }
libloading = "0.8"
ureq = { version = "2", features = ["json"] }
thiserror = "1"
mockito = "1"
```

- [ ] **Step 1.2: Create `tkr-agent` crate manifest**

Create `crates/tkr-agent/Cargo.toml`:

```toml
[package]
name = "tkr-agent"
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"

[lib]
name = "tkr_agent"
crate-type = ["rlib"]

[dependencies]
tkr-api = { path = "../tkr-api" }
tkr-filter = { path = "../tkr-filter" }
anyhow = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
toml = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
mockito = { workspace = true }
```

- [ ] **Step 1.3: Create `tkr-agent/src/lib.rs` skeleton**

```rust
pub mod manifest;
pub mod tool;
pub mod provider;
pub mod loop_;
pub mod receipt;
pub mod tools;

pub use manifest::Manifest;
pub use tool::{Tool, ToolRegistry, ToolResult};
pub use provider::{Provider, Message, ContentBlock, StopReason};
pub use loop_::{run, RunOutcome};
pub use receipt::RunReceipt;
```

- [ ] **Step 1.4: Create `tkr-providers` crate manifest**

Create `crates/tkr-providers/Cargo.toml`:

```toml
[package]
name = "tkr-providers"
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"

[lib]
name = "tkr_providers"
crate-type = ["rlib"]

[dependencies]
tkr-agent = { path = "../tkr-agent" }
anyhow = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
ureq = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
mockito = { workspace = true }
```

- [ ] **Step 1.5: Create `tkr-providers/src/lib.rs` skeleton**

```rust
pub mod anthropic;
pub use anthropic::AnthropicProvider;
```

- [ ] **Step 1.6: Create empty module files so workspace compiles**

Create `crates/tkr-agent/src/manifest.rs`:
```rust
// filled in Task 2
```
Create `crates/tkr-agent/src/tool.rs`:
```rust
// filled in Task 3
```
Create `crates/tkr-agent/src/provider.rs`:
```rust
// filled in Task 5
```
Create `crates/tkr-agent/src/loop_.rs`:
```rust
// filled in Task 8
```
Create `crates/tkr-agent/src/receipt.rs`:
```rust
// filled in Task 9
```
Create `crates/tkr-agent/src/tools/mod.rs`:
```rust
pub mod echo;
```
Create `crates/tkr-agent/src/tools/echo.rs`:
```rust
// filled in Task 4
```
Create `crates/tkr-providers/src/anthropic.rs`:
```rust
// filled in Task 6
```

`lib.rs` re-exports point at types defined in later tasks; we'll wire them up as those tasks land. For Step 1.6, change `tkr-agent/src/lib.rs` to:

```rust
pub mod manifest;
pub mod tool;
pub mod provider;
pub mod loop_;
pub mod receipt;
pub mod tools;
```

(Re-add the `pub use` statements at the end of Task 9.)

- [ ] **Step 1.7: Verify workspace compiles**

Run: `cd /tmp/tkr-work && cargo check --workspace`
Expected: PASS — both new crates compile as empty libraries.

- [ ] **Step 1.8: Commit**

```bash
cd /tmp/tkr-work
git add Cargo.toml crates/tkr-agent crates/tkr-providers
git commit -m "scaffold tkr-agent and tkr-providers crates"
```

---

## Task 2: Manifest types + parser

**Files:**
- Modify: `crates/tkr-agent/src/manifest.rs`

- [ ] **Step 2.1: Write the failing test**

Replace `crates/tkr-agent/src/manifest.rs` with:

```rust
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub model: ModelDecl,
    #[serde(default)]
    pub system: Option<String>,
    pub task: String,
    #[serde(default)]
    pub tools: Vec<ToolDecl>,
    #[serde(default = "default_mode")]
    pub mode: AgentMode,
    #[serde(default = "default_max_steps")]
    pub max_steps: u32,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct ModelDecl {
    pub provider: String,
    pub name: String,
}

#[derive(Debug, Deserialize, PartialEq)]
pub struct ToolDecl {
    pub name: String,
    #[serde(default)]
    pub config: toml::Value,
}

#[derive(Debug, Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    DryRun,
    Approve,
    Auto,
}

fn default_mode() -> AgentMode { AgentMode::Approve }
fn default_max_steps() -> u32 { 20 }

impl Manifest {
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(input)?)
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let src = r#"
name = "hello"
task = "say hi"

[model]
provider = "anthropic"
name = "claude-sonnet-4-6"
"#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.name, "hello");
        assert_eq!(m.task, "say hi");
        assert_eq!(m.model.provider, "anthropic");
        assert_eq!(m.mode, AgentMode::Approve);
        assert_eq!(m.max_steps, 20);
        assert!(m.tools.is_empty());
    }

    #[test]
    fn parses_tools_and_mode() {
        let src = r#"
name = "loud"
task = "echo something"
mode = "auto"
max_steps = 5

[model]
provider = "anthropic"
name = "claude-sonnet-4-6"

[[tools]]
name = "echo"

[[tools]]
name = "echo"
[tools.config]
prefix = "!"
"#;
        let m = Manifest::parse(src).unwrap();
        assert_eq!(m.mode, AgentMode::Auto);
        assert_eq!(m.max_steps, 5);
        assert_eq!(m.tools.len(), 2);
        assert_eq!(m.tools[0].name, "echo");
    }

    #[test]
    fn rejects_missing_required_fields() {
        let src = r#"name = "x""#;
        assert!(Manifest::parse(src).is_err());
    }
}
```

- [ ] **Step 2.2: Run test to verify it fails first, then passes**

Run: `cd /tmp/tkr-work && cargo test -p tkr-agent --lib manifest::`
Expected: PASS — three tests, three passes. (If you want a true red-green: comment out the `Manifest::parse` body, observe failure, restore.)

- [ ] **Step 2.3: Commit**

```bash
cd /tmp/tkr-work
git add crates/tkr-agent/src/manifest.rs
git commit -m "tkr-agent: TOML manifest schema and parser"
```

---

## Task 3: `Tool` trait + `ToolResult`

**Files:**
- Modify: `crates/tkr-agent/src/tool.rs`

- [ ] **Step 3.1: Define the trait and registry**

Replace `crates/tkr-agent/src/tool.rs`:

```rust
use anyhow::Result;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    /// What we hand to the model after filtering.
    pub content: String,
    /// Raw byte length of pre-filter output, for the receipt.
    pub raw_bytes: usize,
    /// Bytes after filtering.
    pub filtered_bytes: usize,
    /// Non-zero exit indicates the tool itself failed; the loop will surface
    /// the error to the model via `is_error: true` in tool_result.
    pub exit: i32,
}

impl ToolResult {
    pub fn is_error(&self) -> bool { self.exit != 0 }
}

pub trait Tool: Send {
    fn name(&self) -> &str;
    /// Anthropic-style JSON schema for the tool's input.
    fn input_schema(&self) -> serde_json::Value;
    fn run(&mut self, input: &serde_json::Value) -> Result<ToolResult>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self { Self { tools: HashMap::new() } }

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
            .map(|t| serde_json::json!({
                "name": t.name(),
                "input_schema": t.input_schema(),
            }))
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Stub;
    impl Tool for Stub {
        fn name(&self) -> &str { "stub" }
        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }
        fn run(&mut self, _input: &serde_json::Value) -> Result<ToolResult> {
            Ok(ToolResult { content: "ok".into(), raw_bytes: 2, filtered_bytes: 2, exit: 0 })
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
```

- [ ] **Step 3.2: Run tests**

Run: `cd /tmp/tkr-work && cargo test -p tkr-agent --lib tool::`
Expected: PASS — two tests pass.

- [ ] **Step 3.3: Commit**

```bash
git add crates/tkr-agent/src/tool.rs
git commit -m "tkr-agent: Tool trait, ToolResult, ToolRegistry"
```

---

## Task 4: `EchoTool`

**Files:**
- Modify: `crates/tkr-agent/src/tools/echo.rs`

- [ ] **Step 4.1: Write the failing test + impl**

Replace `crates/tkr-agent/src/tools/echo.rs`:

```rust
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
    fn name(&self) -> &str { "echo" }

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
        Ok(ToolResult { content: out, raw_bytes: bytes, filtered_bytes: bytes, exit: 0 })
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
```

- [ ] **Step 4.2: Run tests**

Run: `cd /tmp/tkr-work && cargo test -p tkr-agent --lib tools::echo`
Expected: PASS — three tests pass.

- [ ] **Step 4.3: Commit**

```bash
git add crates/tkr-agent/src/tools/echo.rs crates/tkr-agent/src/tools/mod.rs
git commit -m "tkr-agent: EchoTool stub for testing"
```

---

## Task 5: `Provider` trait + message types

**Files:**
- Modify: `crates/tkr-agent/src/provider.rs`

- [ ] **Step 5.1: Define traits and types**

Replace `crates/tkr-agent/src/provider.rs`:

```rust
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
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, #[serde(default)] is_error: bool },
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
```

- [ ] **Step 5.2: Run tests**

Run: `cd /tmp/tkr-work && cargo test -p tkr-agent --lib provider::`
Expected: PASS — two tests pass.

- [ ] **Step 5.3: Commit**

```bash
git add crates/tkr-agent/src/provider.rs
git commit -m "tkr-agent: Provider trait and message types"
```

---

## Task 6: `AnthropicProvider` request shape (offline test)

**Files:**
- Modify: `crates/tkr-providers/src/anthropic.rs`

- [ ] **Step 6.1: Implement request builder + parse, with a test that does NOT hit the network**

Replace `crates/tkr-providers/src/anthropic.rs`:

```rust
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
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

#[derive(Serialize)]
struct _Unused;

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
```

- [ ] **Step 6.2: Run tests**

Run: `cd /tmp/tkr-work && cargo test -p tkr-providers --lib`
Expected: PASS — four tests pass.

- [ ] **Step 6.3: Commit**

```bash
git add crates/tkr-providers/src/anthropic.rs
git commit -m "tkr-providers: Anthropic request builder and response parser"
```

---

## Task 7: `AnthropicProvider::send` against `mockito`

**Files:**
- Create: `crates/tkr-providers/tests/anthropic_http.rs`

- [ ] **Step 7.1: Write a failing HTTP test using `mockito`**

Create `crates/tkr-providers/tests/anthropic_http.rs`:

```rust
use tkr_agent::provider::{ContentBlock, Message, Provider, StopReason};
use tkr_providers::AnthropicProvider;

#[test]
fn send_round_trips_through_mock_server() {
    let mut server = mockito::Server::new();
    let body = r#"{
        "content": [{"type":"text","text":"hi back"}],
        "stop_reason":"end_turn",
        "usage":{"input_tokens":3,"output_tokens":2}
    }"#;
    let _m = server
        .mock("POST", "/v1/messages")
        .match_header("x-api-key", "test-key")
        .match_header("anthropic-version", "2023-06-01")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create();

    let provider = AnthropicProvider::new("test-key", "claude-sonnet-4-6")
        .with_base_url(server.url());
    let msgs = vec![Message::User {
        content: vec![ContentBlock::Text { text: "hi".into() }],
    }];
    let resp = provider.send(None, &msgs, &[], 64).unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    assert_eq!(resp.input_tokens, 3);
    assert_eq!(resp.output_tokens, 2);
}

#[test]
fn send_surfaces_api_error() {
    let mut server = mockito::Server::new();
    let _m = server
        .mock("POST", "/v1/messages")
        .with_status(401)
        .with_body(r#"{"error":{"type":"authentication_error","message":"bad key"}}"#)
        .create();

    let provider = AnthropicProvider::new("nope", "claude-sonnet-4-6")
        .with_base_url(server.url());
    let err = provider.send(None, &[], &[], 16).unwrap_err();
    assert!(err.to_string().contains("401"));
}
```

- [ ] **Step 7.2: Run tests**

Run: `cd /tmp/tkr-work && cargo test -p tkr-providers --test anthropic_http`
Expected: PASS — two tests pass.

- [ ] **Step 7.3: Commit**

```bash
git add crates/tkr-providers/tests/anthropic_http.rs
git commit -m "tkr-providers: HTTP integration test with mockito"
```

---

## Task 8: Agent loop (no filter yet)

**Files:**
- Modify: `crates/tkr-agent/src/loop_.rs`

- [ ] **Step 8.1: Implement the loop**

Replace `crates/tkr-agent/src/loop_.rs`:

```rust
use crate::manifest::Manifest;
use crate::provider::{ContentBlock, Message, Provider, StopReason};
use crate::tool::{ToolRegistry, ToolResult};
use anyhow::{anyhow, Result};

#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub final_text: String,
    pub steps: u32,
    pub input_tokens_total: u32,
    pub output_tokens_total: u32,
    pub raw_bytes_total: usize,
    pub filtered_bytes_total: usize,
}

/// Drive the agent loop until `EndTurn`, `MaxTokens`, or `manifest.max_steps`.
pub fn run(
    manifest: &Manifest,
    provider: &dyn Provider,
    tools: &mut ToolRegistry,
) -> Result<RunOutcome> {
    let schemas = tools.schemas();

    let mut messages: Vec<Message> = vec![Message::User {
        content: vec![ContentBlock::Text { text: manifest.task.clone() }],
    }];

    let mut steps = 0u32;
    let mut input_tokens_total = 0u32;
    let mut output_tokens_total = 0u32;
    let mut raw_bytes_total = 0usize;
    let mut filtered_bytes_total = 0usize;
    let mut final_text = String::new();

    while steps < manifest.max_steps {
        steps += 1;
        let resp = provider.send(
            manifest.system.as_deref(),
            &messages,
            &schemas,
            1024,
        )?;
        input_tokens_total += resp.input_tokens;
        output_tokens_total += resp.output_tokens;

        // Append assistant turn.
        messages.push(Message::Assistant { content: resp.content.clone() });

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
                });
            }
            StopReason::ToolUse => {
                let tool_results = run_tool_calls(&resp.content, tools)?;
                for tr in &tool_results {
                    raw_bytes_total += tr.raw_bytes;
                    filtered_bytes_total += tr.filtered_bytes;
                }
                let blocks: Vec<ContentBlock> = tool_results
                    .into_iter()
                    .map(|(id, res)| ContentBlock::ToolResult {
                        tool_use_id: id,
                        content: res.content,
                        is_error: res.exit != 0,
                    })
                    .collect();
                messages.push(Message::User { content: blocks });
            }
            StopReason::Other(s) => {
                return Err(anyhow!("unexpected stop reason: {}", s));
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
) -> Result<Vec<(String, ToolResult)>> {
    let mut out = Vec::new();
    for b in blocks {
        if let ContentBlock::ToolUse { id, name, input } = b {
            let tool = tools
                .get_mut(name)
                .ok_or_else(|| anyhow!("unknown tool: {}", name))?;
            let res = tool.run(input)?;
            out.push((id.clone(), res));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, ModelDecl, AgentMode};
    use crate::tool::Tool;
    use serde_json::json;
    use std::cell::RefCell;

    /// Provider that returns scripted responses, one per call.
    struct ScriptedProvider {
        script: RefCell<Vec<crate::provider::ProviderResponse>>,
    }
    impl Provider for ScriptedProvider {
        fn send(
            &self,
            _system: Option<&str>,
            _messages: &[Message],
            _tools: &[serde_json::Value],
            _max_tokens: u32,
        ) -> Result<crate::provider::ProviderResponse> {
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
        fn name(&self) -> &str { "echo" }
        fn input_schema(&self) -> serde_json::Value { json!({}) }
        fn run(&mut self, _input: &serde_json::Value) -> Result<ToolResult> {
            let s = "EE\n".to_string();
            Ok(ToolResult { content: s.clone(), raw_bytes: s.len(), filtered_bytes: s.len(), exit: 0 })
        }
    }

    fn manifest() -> Manifest {
        Manifest {
            name: "t".into(),
            model: ModelDecl { provider: "anthropic".into(), name: "x".into() },
            system: None,
            task: "say hi".into(),
            tools: vec![],
            mode: AgentMode::Auto,
            max_steps: 5,
        }
    }

    #[test]
    fn loop_terminates_on_end_turn() {
        let provider = ScriptedProvider {
            script: RefCell::new(vec![crate::provider::ProviderResponse {
                content: vec![ContentBlock::Text { text: "hi back".into() }],
                stop_reason: StopReason::EndTurn,
                input_tokens: 1, output_tokens: 1,
            }]),
        };
        let mut tools = ToolRegistry::new();
        let out = run(&manifest(), &provider, &mut tools).unwrap();
        assert_eq!(out.final_text, "hi back");
        assert_eq!(out.steps, 1);
    }

    #[test]
    fn loop_executes_tool_call_then_finishes() {
        let provider = ScriptedProvider {
            script: RefCell::new(vec![
                crate::provider::ProviderResponse {
                    content: vec![ContentBlock::ToolUse {
                        id: "tu_1".into(),
                        name: "echo".into(),
                        input: json!({}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    input_tokens: 2, output_tokens: 2,
                },
                crate::provider::ProviderResponse {
                    content: vec![ContentBlock::Text { text: "done".into() }],
                    stop_reason: StopReason::EndTurn,
                    input_tokens: 1, output_tokens: 1,
                },
            ]),
        };
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(LoudEcho));
        let out = run(&manifest(), &provider, &mut tools).unwrap();
        assert_eq!(out.steps, 2);
        assert_eq!(out.final_text, "done");
        assert_eq!(out.raw_bytes_total, 3);
    }

    #[test]
    fn loop_caps_at_max_steps() {
        let mut script = Vec::new();
        for _ in 0..10 {
            script.push(crate::provider::ProviderResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tu".into(),
                    name: "echo".into(),
                    input: json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                input_tokens: 0, output_tokens: 0,
            });
        }
        let provider = ScriptedProvider { script: RefCell::new(script) };
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(LoudEcho));
        let mut m = manifest();
        m.max_steps = 3;
        let out = run(&m, &provider, &mut tools).unwrap();
        assert_eq!(out.steps, 3);
    }
}
```

- [ ] **Step 8.2: Run tests**

Run: `cd /tmp/tkr-work && cargo test -p tkr-agent --lib loop_::`
Expected: PASS — three tests pass.

- [ ] **Step 8.3: Commit**

```bash
git add crates/tkr-agent/src/loop_.rs
git commit -m "tkr-agent: agent loop with scripted-provider tests"
```

---

## Task 9: Wire `tkr-filter` into tool output + `RunReceipt`

**Files:**
- Modify: `crates/tkr-agent/src/loop_.rs`
- Modify: `crates/tkr-agent/src/receipt.rs`
- Modify: `crates/tkr-agent/src/lib.rs`

- [ ] **Step 9.1: Implement `RunReceipt` with display test**

Replace `crates/tkr-agent/src/receipt.rs`:

```rust
use crate::loop_::RunOutcome;
use std::fmt;

#[derive(Debug, Clone)]
pub struct RunReceipt {
    pub agent: String,
    pub steps: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub raw_bytes: usize,
    pub filtered_bytes: usize,
}

impl RunReceipt {
    pub fn from_outcome(agent: &str, o: &RunOutcome) -> Self {
        Self {
            agent: agent.to_string(),
            steps: o.steps,
            input_tokens: o.input_tokens_total,
            output_tokens: o.output_tokens_total,
            raw_bytes: o.raw_bytes_total,
            filtered_bytes: o.filtered_bytes_total,
        }
    }

    pub fn savings_ratio(&self) -> f64 {
        if self.raw_bytes == 0 { 0.0 } else {
            1.0 - (self.filtered_bytes as f64 / self.raw_bytes as f64)
        }
    }
}

impl fmt::Display for RunReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "── tkr run receipt ──")?;
        writeln!(f, "  agent:           {}", self.agent)?;
        writeln!(f, "  steps:           {}", self.steps)?;
        writeln!(f, "  tokens (in/out): {} / {}", self.input_tokens, self.output_tokens)?;
        writeln!(
            f,
            "  tool output:     {} B raw → {} B filtered ({:.1}% saved)",
            self.raw_bytes,
            self.filtered_bytes,
            self.savings_ratio() * 100.0
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(raw: usize, filt: usize) -> RunOutcome {
        RunOutcome {
            final_text: "ok".into(),
            steps: 2,
            input_tokens_total: 100,
            output_tokens_total: 50,
            raw_bytes_total: raw,
            filtered_bytes_total: filt,
        }
    }

    #[test]
    fn savings_ratio_handles_zero() {
        let r = RunReceipt::from_outcome("a", &outcome(0, 0));
        assert_eq!(r.savings_ratio(), 0.0);
    }

    #[test]
    fn savings_ratio_basic() {
        let r = RunReceipt::from_outcome("a", &outcome(1000, 200));
        assert!((r.savings_ratio() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn display_includes_agent_and_savings() {
        let r = RunReceipt::from_outcome("hello", &outcome(1000, 200));
        let s = format!("{}", r);
        assert!(s.contains("hello"));
        assert!(s.contains("80.0% saved"));
    }
}
```

- [ ] **Step 9.2: Add filter integration to the loop**

Modify `crates/tkr-agent/src/loop_.rs`. Change the public `run` function signature to accept an optional filter, and pass tool output through it before recording bytes.

Replace the entire `pub fn run(...)` and `run_tool_calls(...)` portion with:

```rust
use tkr_filter::FilterPlugin;
use tkr_api::{FilterResult, Plugin};

pub fn run(
    manifest: &Manifest,
    provider: &dyn Provider,
    tools: &mut ToolRegistry,
    filter: Option<&mut FilterPlugin>,
) -> Result<RunOutcome> {
    let schemas = tools.schemas();
    let mut messages: Vec<Message> = vec![Message::User {
        content: vec![ContentBlock::Text { text: manifest.task.clone() }],
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
        messages.push(Message::Assistant { content: resp.content.clone() });

        match resp.stop_reason {
            StopReason::EndTurn | StopReason::MaxTokens => {
                final_text = collect_text(&resp.content);
                return Ok(RunOutcome {
                    final_text, steps, input_tokens_total, output_tokens_total,
                    raw_bytes_total, filtered_bytes_total,
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
            StopReason::Other(s) => return Err(anyhow!("unexpected stop reason: {}", s)),
        }
    }

    Ok(RunOutcome {
        final_text, steps, input_tokens_total, output_tokens_total,
        raw_bytes_total, filtered_bytes_total,
    })
}

fn run_tool_calls(
    blocks: &[ContentBlock],
    tools: &mut ToolRegistry,
) -> Result<Vec<(String, ToolResult, String)>> {
    let mut out = Vec::new();
    for b in blocks {
        if let ContentBlock::ToolUse { id, name, input } = b {
            let tool = tools.get_mut(name).ok_or_else(|| anyhow!("unknown tool: {}", name))?;
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
            FilterResult::Pass => { out.push_str(line); out.push('\n'); }
            FilterResult::Suppress | FilterResult::SuppressWithNote(_) => {}
            FilterResult::Replace(p, len) => {
                let bytes = unsafe { std::slice::from_raw_parts(p as *const u8, len) };
                if let Ok(s) = std::str::from_utf8(bytes) {
                    out.push_str(s); out.push('\n');
                }
                unsafe { let _ = Box::from_raw(p as *mut u8); }
            }
            FilterResult::Annotate(_, _) => { out.push_str(line); out.push('\n'); }
        }
    }
    let summary = plugin.flush();
    if !summary.is_empty() { out.push_str(&summary); }
    out
}
```

Update existing loop tests in the same file to pass `None` as the new filter argument (`run(&manifest(), &provider, &mut tools, None)`).

- [ ] **Step 9.3: Add a test that exercises the filter path**

Add to the `tests` module in `loop_.rs`:

```rust
#[test]
fn filter_compresses_tool_output() {
    use tkr_filter::FilterPlugin;
    let filter_toml = r#"
command = "echo"
[[rules]]
match = "^DROP "
action = "suppress"
"#;
    let mut filter = FilterPlugin::from_toml(filter_toml).unwrap();

    struct NoisyEcho;
    impl Tool for NoisyEcho {
        fn name(&self) -> &str { "echo" }
        fn input_schema(&self) -> serde_json::Value { json!({}) }
        fn run(&mut self, _input: &serde_json::Value) -> Result<ToolResult> {
            let s = "KEEP one\nDROP two\nKEEP three\n".to_string();
            Ok(ToolResult { content: s.clone(), raw_bytes: s.len(), filtered_bytes: s.len(), exit: 0 })
        }
    }

    let provider = ScriptedProvider {
        script: RefCell::new(vec![
            crate::provider::ProviderResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tu".into(), name: "echo".into(), input: json!({}),
                }],
                stop_reason: StopReason::ToolUse,
                input_tokens: 0, output_tokens: 0,
            },
            crate::provider::ProviderResponse {
                content: vec![ContentBlock::Text { text: "ok".into() }],
                stop_reason: StopReason::EndTurn,
                input_tokens: 0, output_tokens: 0,
            },
        ]),
    };
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(NoisyEcho));
    let out = run(&manifest(), &provider, &mut tools, Some(&mut filter)).unwrap();
    assert!(out.filtered_bytes_total < out.raw_bytes_total);
}
```

You will need to confirm the `tkr-filter` rule schema by skimming `crates/tkr-filter/src/rules.rs`. If its `Rule` requires fields that differ from `match`/`action` above, adjust the test's filter TOML accordingly. **Stop and read that file before completing this step** — the schema is the source of truth, and a wrong rule TOML here will fail the test.

- [ ] **Step 9.4: Restore `lib.rs` re-exports**

Replace `crates/tkr-agent/src/lib.rs`:

```rust
pub mod manifest;
pub mod tool;
pub mod provider;
pub mod loop_;
pub mod receipt;
pub mod tools;

pub use manifest::Manifest;
pub use tool::{Tool, ToolRegistry, ToolResult};
pub use provider::{Provider, Message, ContentBlock, StopReason, ProviderResponse};
pub use loop_::{run, RunOutcome};
pub use receipt::RunReceipt;
```

- [ ] **Step 9.5: Run all tkr-agent tests**

Run: `cd /tmp/tkr-work && cargo test -p tkr-agent`
Expected: PASS — all unit tests pass, including the new filter-compression test.

- [ ] **Step 9.6: Commit**

```bash
git add crates/tkr-agent/src/loop_.rs crates/tkr-agent/src/receipt.rs crates/tkr-agent/src/lib.rs
git commit -m "tkr-agent: tkr-filter on tool output + RunReceipt"
```

---

## Task 10: CLI subcommand `tkr agent run`

**Files:**
- Modify: `crates/tkr/Cargo.toml`
- Modify: `crates/tkr/src/cli.rs`
- Modify: `crates/tkr/src/main.rs` (or `dispatch.rs` — whichever currently routes `Commands`)

- [ ] **Step 10.1: Add new deps to `tkr` binary crate**

Edit `crates/tkr/Cargo.toml`. In `[dependencies]`, add:

```toml
tkr-agent = { path = "../tkr-agent" }
tkr-providers = { path = "../tkr-providers" }
```

- [ ] **Step 10.2: Add `Agent { Run { manifest } }` subcommand**

Edit `crates/tkr/src/cli.rs`. Replace the `Commands` enum with:

```rust
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Live token savings dashboard
    Watch,
    /// Show token savings analytics
    Gain {
        #[arg(long)]
        breakdown: bool,
    },
    /// Analyze session history for missed savings
    Discover,
    /// Run an agent from a TOML manifest
    Agent {
        #[command(subcommand)]
        cmd: AgentCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Execute one agent run
    Run {
        /// Path to a TOML manifest
        manifest: std::path::PathBuf,
    },
}
```

- [ ] **Step 10.3: Wire dispatch**

Find the existing match on `Commands` in `main.rs` or `dispatch.rs`. Add an arm:

```rust
Commands::Agent { cmd } => match cmd {
    AgentCmd::Run { manifest } => run_agent(&manifest)?,
},
```

Add a `run_agent` function to the same file (or a new module — the simplest path is a sibling module file `crates/tkr/src/agent_cmd.rs`). Create `crates/tkr/src/agent_cmd.rs`:

```rust
use anyhow::{anyhow, Context, Result};
use std::path::Path;
use tkr_agent::{
    tools::echo::EchoTool, Manifest, RunReceipt, ToolRegistry,
};
use tkr_providers::AnthropicProvider;

pub fn run_agent(manifest_path: &Path) -> Result<()> {
    let manifest = Manifest::load(manifest_path)
        .with_context(|| format!("loading manifest {}", manifest_path.display()))?;

    let mut tools = ToolRegistry::new();
    for decl in &manifest.tools {
        match decl.name.as_str() {
            "echo" => tools.register(Box::new(EchoTool)),
            other => return Err(anyhow!("unknown tool '{}' (v1 only ships 'echo')", other)),
        }
    }

    let provider = match manifest.model.provider.as_str() {
        "anthropic" => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| anyhow!("ANTHROPIC_API_KEY not set"))?;
            AnthropicProvider::new(key, &manifest.model.name)
        }
        other => return Err(anyhow!("unknown provider '{}' (v1 only ships 'anthropic')", other)),
    };

    let outcome = tkr_agent::run(&manifest, &provider, &mut tools, None)?;
    println!("{}", outcome.final_text);
    println!();
    println!("{}", RunReceipt::from_outcome(&manifest.name, &outcome));
    Ok(())
}
```

Add `mod agent_cmd;` and `use cli::AgentCmd;` (and `use agent_cmd::run_agent;`) at the top of `main.rs` (or wherever the dispatch lives).

- [ ] **Step 10.4: Build the binary**

Run: `cd /tmp/tkr-work && cargo build -p tkr`
Expected: PASS — binary builds.

- [ ] **Step 10.5: Commit**

```bash
git add crates/tkr/Cargo.toml crates/tkr/src/cli.rs crates/tkr/src/main.rs crates/tkr/src/agent_cmd.rs
git commit -m "tkr CLI: add 'agent run' subcommand"
```

---

## Task 11: Example manifest + integration test

**Files:**
- Create: `examples/hello.toml`
- Create: `crates/tkr-agent/tests/loop_integration.rs`

- [ ] **Step 11.1: Add example manifest**

Create `examples/hello.toml`:

```toml
name = "hello"
task = "Use the echo tool to say 'hello world', then stop."
mode = "auto"
max_steps = 4

[model]
provider = "anthropic"
name = "claude-sonnet-4-6"

[[tools]]
name = "echo"
```

- [ ] **Step 11.2: End-to-end integration test against a mock HTTP server**

Add `mockito` and `tkr-providers` to `crates/tkr-agent/Cargo.toml` `[dev-dependencies]`:

```toml
[dev-dependencies]
mockito = { workspace = true }
tkr-providers = { path = "../tkr-providers" }
```

Create `crates/tkr-agent/tests/loop_integration.rs`:

```rust
use tkr_agent::{tools::echo::EchoTool, Manifest, ToolRegistry};
use tkr_providers::AnthropicProvider;

#[test]
fn end_to_end_echo_run() {
    let mut server = mockito::Server::new();

    // Turn 1: model asks to use echo.
    let m1 = server.mock("POST", "/v1/messages")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{
            "content":[{"type":"tool_use","id":"tu_1","name":"echo","input":{"text":"hello world"}}],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":5,"output_tokens":10}
        }"#)
        .expect(1)
        .create();

    // Turn 2: model finishes.
    let m2 = server.mock("POST", "/v1/messages")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{
            "content":[{"type":"text","text":"echoed"}],
            "stop_reason":"end_turn",
            "usage":{"input_tokens":3,"output_tokens":2}
        }"#)
        .expect(1)
        .create();

    let manifest_src = r#"
name = "hello"
task = "say hi"
mode = "auto"
max_steps = 4

[model]
provider = "anthropic"
name = "claude-sonnet-4-6"

[[tools]]
name = "echo"
"#;
    let manifest = Manifest::parse(manifest_src).unwrap();
    let provider = AnthropicProvider::new("k", "claude-sonnet-4-6")
        .with_base_url(server.url());

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(EchoTool));

    let outcome = tkr_agent::run(&manifest, &provider, &mut tools, None).unwrap();
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "echoed");
    assert!(outcome.raw_bytes_total > 0); // echo produced bytes
    m1.assert();
    m2.assert();
}
```

- [ ] **Step 11.3: Run the integration test**

Run: `cd /tmp/tkr-work && cargo test -p tkr-agent --test loop_integration`
Expected: PASS — single test passes.

- [ ] **Step 11.4: Commit**

```bash
git add examples/hello.toml crates/tkr-agent/Cargo.toml crates/tkr-agent/tests/loop_integration.rs
git commit -m "tkr-agent: end-to-end integration test + hello.toml example"
```

---

## Task 12: Smoke-test the binary against a real provider (manual)

**Files:** none

- [ ] **Step 12.1: Build release binary**

Run: `cd /tmp/tkr-work && cargo build --release -p tkr`
Expected: PASS, binary at `target/release/tkr`.

- [ ] **Step 12.2: Set API key and run**

Run:
```bash
export ANTHROPIC_API_KEY=<your real key>
cd /tmp/tkr-work
./target/release/tkr agent run examples/hello.toml
```

Expected output: model emits a tool_use for `echo` with `text: "hello world"`, the echo tool returns it, the model emits final text, and you see something like:

```
hello world echoed.

── tkr run receipt ──
  agent:           hello
  steps:           2
  tokens (in/out): 12 / 7
  tool output:     12 B raw → 12 B filtered (0.0% saved)
```

(Exact tokens vary; savings is 0% in v1 because no filter is wired into the binary yet — that's the next plan slice.)

- [ ] **Step 12.3: If smoke test passes, push the branch**

Run:
```bash
cd /tmp/tkr-work
git push -u origin spec/agents-platform
```
Expected: branch pushed; PR can be opened against `main`.

---

## Self-Review Notes (already applied)

- **Spec coverage:** §7.3 component layout (`tkr-agent`, `tkr-providers` ✓), §7.4 data flow (manifest → loader → loop → tool → filter → tool_result ✓), §8 v1 in-scope items: agent run ✓, Anthropic provider ✓, `tkr-filter` egress ✓, run receipt ✓. Out-of-scope: cron daemon, OpenAI provider, sandbox, vault, signing, tools beyond `echo`, dashboard — all deferred to Plans 2–6.
- **Placeholders:** none. Every step has runnable code.
- **Type consistency:** `RunOutcome` field names (`raw_bytes_total`, `filtered_bytes_total`) match between Task 8 and Task 9. `Manifest` field names match between Tasks 2 and 10. `AgentMode` not yet enforced at runtime — that's deliberate (sandbox enforcement comes in Plan 2). `ToolResult.exit` consistent across Tasks 3, 4, 8, 9. Filter-rule schema (`match` / `action = "suppress"`) in Task 9.3 is asserted by reading `tkr-filter/src/rules.rs` — explicit instruction in that step.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-28-plan-1-tkr-agent-runtime-mvp.md`. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks, fast iteration.
2. **Inline Execution** — execute tasks in this session with batch checkpoints.

Which approach?
