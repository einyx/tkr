use jkr_agent::{tools::echo::EchoTool, Manifest, ToolRegistry};
use jkr_providers::AnthropicProvider;

#[test]
fn end_to_end_echo_run() {
    let mut server = mockito::Server::new();

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

    let m2 = server
        .mock("POST", "/v1/messages")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
            "content":[{"type":"text","text":"echoed"}],
            "stop_reason":"end_turn",
            "usage":{"input_tokens":3,"output_tokens":2}
        }"#,
        )
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
    let provider = AnthropicProvider::new("k", "claude-sonnet-4-6").with_base_url(server.url());

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(EchoTool));

    let outcome = jkr_agent::run(&manifest, &provider, &mut tools, None, None).unwrap();
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "echoed");
    assert!(outcome.raw_bytes_total > 0);
    m1.assert();
    m2.assert();
}

#[test]
fn emits_step_tool_and_done_events() {
    let mut server = mockito::Server::new();

    let _m1 = server.mock("POST", "/v1/messages")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{
            "content":[{"type":"tool_use","id":"tu_1","name":"echo","input":{"text":"hello world"}}],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":5,"output_tokens":10}
        }"#)
        .expect(1)
        .create();

    let _m2 = server
        .mock("POST", "/v1/messages")
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
    let provider = AnthropicProvider::new("k", "claude-sonnet-4-6").with_base_url(server.url());

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(EchoTool));

    let mut sink = jkr_agent::VecSink::default();
    let outcome = jkr_agent::run(&manifest, &provider, &mut tools, None, Some(&mut sink)).unwrap();

    let kinds: Vec<&str> = sink.events.iter().map(|e| match &e.kind {
        jkr_api::AgentEventKind::Step { .. } => "step",
        jkr_api::AgentEventKind::Token { .. } => "token",
        jkr_api::AgentEventKind::ToolStart { .. } => "tool_start",
        jkr_api::AgentEventKind::ToolOutput { .. } => "tool_output",
        jkr_api::AgentEventKind::Done { .. } => "done",
        jkr_api::AgentEventKind::Error { .. } => "error",
    }).collect();

    assert_eq!(kinds, vec!["step", "tool_start", "tool_output", "step", "token", "done"]);
    assert!(kinds.contains(&"token"));
    for (i, e) in sink.events.iter().enumerate() {
        assert_eq!(e.seq, i as u64);
    }
    assert!(outcome.steps >= 1);
}
