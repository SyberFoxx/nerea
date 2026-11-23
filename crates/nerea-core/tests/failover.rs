//! Composition check: a real transport behind a real failover chain.
//!
//! Unit tests cover the transport and the failover logic separately; this proves
//! they are wired together, which is the behaviour users actually get.

use std::sync::Arc;

use futures::StreamExt;
use nerea_core::{AiProvider, ChatMessage, FallbackProvider, OpenAiCompatProvider};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn spawn_mock(code: u16, reason: &'static str, content_type: &'static str, body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock listener");
    let addr = listener.local_addr().expect("mock addr");

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = [0u8; 8192];
        let _ = socket.read(&mut buffer).await;

        let head = format!(
            "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(body.as_bytes()).await;
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}

fn provider(id: &str, base_url: String) -> Arc<dyn AiProvider> {
    Arc::new(
        OpenAiCompatProvider::new(id, base_url, "test-key", "test-model")
            .expect("provider"),
    )
}

#[tokio::test]
async fn a_broken_primary_answers_through_the_secondary() {
    let dead = spawn_mock(503, "Service Unavailable", "application/json", r#"{"error":{"message":"down"}}"#.to_owned()).await;
    let alive = spawn_mock(
        200,
        "OK",
        "application/json",
        r#"{"model":"deepseek-v4-flash","choices":[{"message":{"content":"desde deepseek"},"finish_reason":"stop"}]}"#.to_owned(),
    )
    .await;

    let chain = FallbackProvider::new(provider("opencode-go", dead), provider("deepseek", alive));
    let response = chain
        .chat_completion(&[ChatMessage::user("hola")])
        .await
        .expect("secondary answer");

    assert_eq!(response.content, "desde deepseek");
    assert_eq!(response.provider, "deepseek", "failover must be observable");
}

#[tokio::test]
async fn streaming_fails_over_when_the_primary_refuses_the_request() {
    let dead = spawn_mock(429, "Too Many Requests", "application/json", r#"{"error":{"message":"slow down"}}"#.to_owned()).await;
    let alive = spawn_mock(
        200,
        "OK",
        "text/event-stream",
        concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hola \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"mundo\"}}]}\n\n",
            "data: [DONE]\n\n",
        )
        .to_owned(),
    )
    .await;

    let chain = FallbackProvider::new(provider("opencode-go", dead), provider("deepseek", alive));
    let mut stream = chain
        .stream_chat_completion(&[ChatMessage::user("hola")])
        .await
        .expect("secondary stream");

    let mut text = String::new();
    while let Some(item) = stream.next().await {
        text.push_str(&item.expect("token"));
    }
    assert_eq!(text, "hola mundo");
}

/// Declares a longer body than it sends, then closes: the transport sees a decode error.
async fn spawn_truncated_stream(declared_len: usize, partial: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock listener");
    let addr = listener.local_addr().expect("mock addr");

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = [0u8; 8192];
        let _ = socket.read(&mut buffer).await;

        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
        );
        let _ = socket.write_all(head.as_bytes()).await;
        let _ = socket.write_all(partial.as_bytes()).await;
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}

#[tokio::test]
async fn a_stream_dropped_after_a_role_chunk_fails_over() {
    // The role-only chunk decodes to an empty delta: it must not count as output.
    let partial = "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\ndata: {\"choices\":[{\"delta\":{";
    let broken = spawn_truncated_stream(4096, partial).await;
    let alive = spawn_mock(
        200,
        "OK",
        "text/event-stream",
        "data: {\"choices\":[{\"delta\":{\"content\":\"desde deepseek\"}}]}\n\ndata: [DONE]\n\n"
            .to_owned(),
    )
    .await;

    let chain = FallbackProvider::new(provider("opencode-go", broken), provider("deepseek", alive));
    let mut stream = chain
        .stream_chat_completion(&[ChatMessage::user("hola")])
        .await
        .expect("secondary stream");

    let mut text = String::new();
    while let Some(item) = stream.next().await {
        text.push_str(&item.expect("token"));
    }
    assert_eq!(text, "desde deepseek");
}
