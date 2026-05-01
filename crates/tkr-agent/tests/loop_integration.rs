use tkr_agent::{tools::echo::EchoTool, Manifest, ToolRegistry};
use tkr_providers::AnthropicProvider;

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

    let outcome = tkr_agent::run(&manifest, &provider, &mut tools, None).unwrap();
    assert_eq!(outcome.steps, 2);
    assert_eq!(outcome.final_text, "echoed");
    assert!(outcome.raw_bytes_total > 0);
    m1.assert();
    m2.assert();
}
