use jkr_agent::provider::{ContentBlock, Message, Provider, StopReason};
use jkr_providers::AnthropicProvider;

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

    let provider =
        AnthropicProvider::new("test-key", "claude-sonnet-4-6").with_base_url(server.url());
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

    let provider = AnthropicProvider::new("nope", "claude-sonnet-4-6").with_base_url(server.url());
    let err = provider.send(None, &[], &[], 16).unwrap_err();
    assert!(err.to_string().contains("401"));
}
