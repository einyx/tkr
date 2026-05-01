use tkr_agent::provider::{ContentBlock, Message, Provider, StopReason};
use tkr_providers::OllamaProvider;

#[test]
fn send_round_trips_through_mock_server() {
    let mut server = mockito::Server::new();
    let body = r#"{
        "choices":[{"message":{"content":"hi back"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":3,"completion_tokens":2}
    }"#;
    let _m = server
        .mock("POST", "/v1/chat/completions")
        .match_header("content-type", "application/json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create();

    let provider = OllamaProvider::new("qwen2.5-coder:7b").with_base_url(server.url());
    let msgs = vec![Message::User {
        content: vec![ContentBlock::Text { text: "hi".into() }],
    }];
    let resp = provider.send(None, &msgs, &[], 64).unwrap();
    assert_eq!(resp.stop_reason, StopReason::EndTurn);
    assert_eq!(resp.input_tokens, 3);
    assert_eq!(resp.output_tokens, 2);
    match &resp.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "hi back"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn send_surfaces_api_error() {
    let mut server = mockito::Server::new();
    let _m = server
        .mock("POST", "/v1/chat/completions")
        .with_status(404)
        .with_body(r#"{"error":"model not found"}"#)
        .create();

    let provider = OllamaProvider::new("does-not-exist").with_base_url(server.url());
    let err = provider.send(None, &[], &[], 16).unwrap_err();
    assert!(err.to_string().contains("404"));
}
