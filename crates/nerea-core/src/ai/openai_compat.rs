use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use super::{AiProvider, ChatMessage, ChatResponse, ChatStream, Usage};
use crate::error::ProviderError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// No `Debug` on purpose: deriving it would print the API key.
pub struct OpenAiCompatProvider {
    client: Client,
    base_url: String,
    api_key: String,
    id: String,
    model: String,
}

impl OpenAiCompatProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        let client = Client::builder().connect_timeout(CONNECT_TIMEOUT).build()?;

        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            id: id.into(),
            model: model.into(),
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
}

// ---------- wire types: private, never escape this module ----------

#[derive(Deserialize)]
struct CompletionBody {
    #[serde(default)]
    model: String,
    #[serde(default)]
    choices: Vec<CompletionChoice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    #[serde(default)]
    message: Option<ResponseMessage>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: Option<StreamDelta>,
}

#[derive(Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
    code: Option<String>,
}

/// Providers disagree on the error envelope, so fall back to the raw body.
fn api_error(status: StatusCode, body: &str) -> ProviderError {
    match serde_json::from_str::<ErrorEnvelope>(body) {
        Ok(envelope) => ProviderError::Api {
            status: status.as_u16(),
            code: envelope.error.code,
            message: envelope.error.message,
        },
        Err(_) => ProviderError::Api {
            status: status.as_u16(),
            code: None,
            message: body.chars().take(300).collect(),
        },
    }
}

#[async_trait]
impl AiProvider for OpenAiCompatProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
    ) -> Result<ChatResponse, ProviderError> {
        let body = ChatRequest { model: &self.model, messages, stream: false };

        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .timeout(REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await?;
            return Err(api_error(status, &text));
        }

        let completion: CompletionBody = response.json().await?;
        let choice = completion
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Protocol("response contained no choices".into()))?;
        let CompletionChoice { message, finish_reason } = choice;

        Ok(ChatResponse {
            content: message.and_then(|msg| msg.content).unwrap_or_default(),
            model: if completion.model.is_empty() {
                self.model.clone()
            } else {
                completion.model
            },
            finish_reason,
            usage: completion.usage,
        })
    }

    async fn stream_chat_completion(
        &self,
        messages: &[ChatMessage],
    ) -> Result<ChatStream, ProviderError> {
        let body = ChatRequest { model: &self.model, messages, stream: true };

        // No total timeout here: an SSE body can legitimately stay open for minutes.
        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await?;
            return Err(api_error(status, &text));
        }

        Ok(parse_sse(response.bytes_stream()))
    }
}

fn parse_sse<S>(body: S) -> ChatStream
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let body = Box::pin(body);
    Box::pin(stream::unfold(
        (body, Vec::new()),
        |(mut body, mut buffer)| async move {
            loop {
                // Events end at a blank line; `\n\n` can never split a UTF-8 char.
                if let Some(end) = buffer.windows(2).position(|w| w == b"\n\n") {
                    let event = String::from_utf8_lossy(&buffer[..end]).into_owned();
                    buffer.drain(..end + 2);

                    match sse_data(&event) {
                        Some(data) if data.trim() == "[DONE]" => return None,
                        Some(data) => return Some((decode_delta(&data), (body, buffer))),
                        None => continue,
                    }
                }

                match body.as_mut().next().await {
                    Some(Ok(chunk)) => buffer.extend_from_slice(&chunk),
                    Some(Err(err)) => {
                        return Some((Err(ProviderError::Network(err)), (body, buffer)));
                    }
                    None => return None,
                }
            }
        },
    ))
}

/// Collects every `data:` line of one event, per the SSE spec.
fn sse_data(block: &str) -> Option<String> {
    let mut data = String::new();
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim());
        }
    }
    (!data.is_empty()).then_some(data)
}

/// Returns an empty string for role-only chunks; callers filter those out.
fn decode_delta(data: &str) -> Result<String, ProviderError> {
    let chunk: StreamChunk = serde_json::from_str(data)?;
    Ok(chunk
        .choices
        .first()
        .and_then(|choice| choice.delta.as_ref())
        .and_then(|delta| delta.content.clone())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serves one canned response, written in the given parts.
    async fn spawn_mock(content_type: &'static str, parts: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock listener");
        let addr = listener.local_addr().expect("mock addr");

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf).await;

            let len: usize = parts.iter().map(String::len).sum();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
            );
            let _ = socket.write_all(head.as_bytes()).await;
            for part in parts {
                let _ = socket.write_all(part.as_bytes()).await;
            }
            let _ = socket.shutdown().await;
        });

        format!("http://{addr}")
    }

    fn provider(base_url: String) -> OpenAiCompatProvider {
        OpenAiCompatProvider::new("mock", base_url, "test-key", "mock-model").expect("provider")
    }

    #[tokio::test]
    async fn chat_completion_parses_normalized_response() {
        let body = r#"{"model":"mock-model-2026","choices":[{"message":{"role":"assistant","content":"Hola"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#;
        let url = spawn_mock("application/json", vec![body.to_owned()]).await;

        let response = provider(url)
            .chat_completion(&[ChatMessage::user("hi")])
            .await
            .expect("call");

        assert_eq!(response.content, "Hola");
        assert_eq!(response.model, "mock-model-2026");
        assert_eq!(response.finish_reason.as_deref(), Some("stop"));
        assert_eq!(response.usage.map(|usage| usage.total_tokens), Some(5));
    }

    #[tokio::test]
    async fn stream_chat_completion_yields_deltas_across_split_chunks() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\", \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        // Cut mid-event so the buffering logic is actually exercised.
        let parts = vec![
            body[..20].to_owned(),
            body[20..58].to_owned(),
            body[58..].to_owned(),
        ];
        let url = spawn_mock("text/event-stream", parts).await;

        let mut stream = provider(url)
            .stream_chat_completion(&[ChatMessage::user("hi")])
            .await
            .expect("stream");

        let mut collected = String::new();
        while let Some(item) = stream.next().await {
            collected.push_str(&item.expect("chunk"));
        }
        assert_eq!(collected, "Hello, world");
    }

    #[test]
    fn sse_data_ignores_non_data_lines() {
        assert_eq!(sse_data("event: ping\n: keep-alive"), None);
        assert_eq!(sse_data("data: {\"a\":1}").as_deref(), Some("{\"a\":1}"));
    }

    #[test]
    fn api_error_falls_back_to_raw_body() {
        match api_error(StatusCode::UNAUTHORIZED, "plain text") {
            ProviderError::Api { status, code, message } => {
                assert_eq!(status, 401);
                assert_eq!(code, None);
                assert_eq!(message, "plain text");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
