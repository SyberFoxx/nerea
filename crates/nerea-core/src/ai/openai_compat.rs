use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use reqwest::header::HeaderName;

use super::{AiProvider, ChatMessage, ChatRequest, ChatResponse, ChatStream, Usage};
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
    session_header: Option<HeaderName>,
}

impl OpenAiCompatProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        // Gateways ask clients to identify themselves instead of posing as a
        // generic SDK; some reject requests without it.
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("nerea/", env!("CARGO_PKG_VERSION")))
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
            id: id.into(),
            model: model.into(),
            session_header: None,
        })
    }

    /// Some gateways reject requests without a per-conversation session id.
    pub fn with_session_header(mut self, header: &'static str) -> Result<Self, ProviderError> {
        let name = HeaderName::try_from(header)
            .map_err(|_| ProviderError::Protocol(format!("invalid session header `{header}`")))?;
        self.session_header = Some(name);
        Ok(self)
    }

    fn request_builder(&self, session_id: &str) -> reqwest::RequestBuilder {
        let builder = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.api_key);

        match &self.session_header {
            Some(header) => builder.header(header, session_id),
            None => builder,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

#[derive(Serialize)]
struct ChatBody<'a> {
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

    async fn chat_completion(&self, request: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let body = ChatBody {
            model: &self.model,
            messages: &request.messages,
            stream: false,
        };

        let response = self
            .request_builder(&request.session_id)
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
            provider: self.id.clone(),
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
        request: &ChatRequest,
    ) -> Result<ChatStream, ProviderError> {
        let body = ChatBody {
            model: &self.model,
            messages: &request.messages,
            stream: true,
        };

        // No total timeout here: an SSE body can legitimately stay open for minutes.
        let response = self
            .request_builder(&request.session_id)
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
        (body, Vec::new(), false),
        |(mut body, mut buffer, mut exhausted)| async move {
            loop {
                if let Some((end, separator)) = event_boundary(&buffer) {
                    let event = String::from_utf8_lossy(&buffer[..end]).into_owned();
                    buffer.drain(..end + separator);

                    match classify(&event) {
                        SseEvent::Done => return None,
                        SseEvent::Ignore => continue,
                        SseEvent::Delta(item) => return Some((item, (body, buffer, exhausted))),
                    }
                }

                if exhausted {
                    // Flush a final event that arrived without its blank line.
                    if buffer.is_empty() {
                        return None;
                    }
                    let event = String::from_utf8_lossy(&buffer).into_owned();
                    buffer.clear();
                    return match classify(&event) {
                        SseEvent::Delta(item) => Some((item, (body, buffer, exhausted))),
                        SseEvent::Done | SseEvent::Ignore => None,
                    };
                }

                match body.as_mut().next().await {
                    Some(Ok(chunk)) => buffer.extend_from_slice(&chunk),
                    Some(Err(err)) => {
                        return Some((Err(ProviderError::Network(err)), (body, buffer, exhausted)));
                    }
                    None => exhausted = true,
                }
            }
        },
    ))
}

enum SseEvent {
    Delta(Result<String, ProviderError>),
    Done,
    Ignore,
}

fn classify(event: &str) -> SseEvent {
    match sse_data(event) {
        None => SseEvent::Ignore,
        Some(data) if data.trim() == "[DONE]" => SseEvent::Done,
        Some(data) => SseEvent::Delta(decode_delta(&data)),
    }
}

/// Offset of the blank line ending the event, plus its length.
/// Accepts both `\n\n` and the CRLF form allowed by the SSE spec.
fn event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|w| w == b"\n\n").map(|at| (at, 2));
    let crlf = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|at| (at, 4));

    match (lf, crlf) {
        (Some(lf), Some(crlf)) if crlf.0 < lf.0 => Some(crlf),
        (Some(lf), _) => Some(lf),
        (None, Some(crlf)) => Some(crlf),
        (None, None) => None,
    }
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

    fn request() -> ChatRequest {
        ChatRequest {
            messages: vec![ChatMessage::user("hi")],
            session_id: "test-session".to_owned(),
        }
    }

    fn provider(base_url: String) -> OpenAiCompatProvider {
        OpenAiCompatProvider::new("mock", base_url, "test-key", "mock-model").expect("provider")
    }

    #[tokio::test]
    async fn chat_completion_parses_normalized_response() {
        let body = r#"{"model":"mock-model-2026","choices":[{"message":{"role":"assistant","content":"Hola"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#;
        let url = spawn_mock("application/json", vec![body.to_owned()]).await;

        let response = provider(url)
            .chat_completion(&request())
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
            .stream_chat_completion(&request())
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

    /// Rejects the request unless the gateway contract headers are present.
    async fn spawn_header_guard(session: &'static str, expect_session: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock listener");
        let addr = listener.local_addr().expect("mock addr");

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0u8; 8192];
            let read = socket.read(&mut buffer).await.unwrap_or(0);
            let head = String::from_utf8_lossy(&buffer[..read]).to_lowercase();

            let has_session = head.contains(&format!("x-opencode-session: {session}"));
            let has_agent = head.contains("user-agent: nerea/");
            let ok = has_agent && has_session == expect_session;

            let (code, reason, body) = if ok {
                (200, "OK", r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_owned())
            } else {
                (400, "Bad Request", r#"{"error":{"message":"bad headers"}}"#.to_owned())
            };

            let head = format!(
                "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.shutdown().await;
        });

        format!("http://{addr}")
    }

    #[tokio::test]
    async fn sends_the_user_agent_and_session_header_the_gateway_requires() {
        let url = spawn_header_guard("sess-42", true).await;
        let provider = provider(url)
            .with_session_header("x-opencode-session")
            .expect("valid header");

        let response = provider
            .chat_completion(&ChatRequest {
                messages: vec![ChatMessage::user("hi")],
                session_id: "sess-42".to_owned(),
            })
            .await
            .expect("gateway accepted the request");

        assert_eq!(response.content, "ok");
    }

    #[tokio::test]
    async fn does_not_leak_the_session_id_to_providers_that_ignore_it() {
        let url = spawn_header_guard("sess-42", false).await;

        // DeepSeek-style: no session header configured, so it must not be sent.
        let response = provider(url)
            .chat_completion(&ChatRequest {
                messages: vec![ChatMessage::user("hi")],
                session_id: "sess-42".to_owned(),
            })
            .await
            .expect("request without session header");

        assert_eq!(response.content, "ok");
    }

    #[test]
    fn sse_data_joins_multiple_data_lines() {
        assert_eq!(
            sse_data("data: {\"a\":\ndata: 1}").as_deref(),
            Some("{\"a\":\n1}")
        );
    }

    fn chunked(parts: Vec<&'static [u8]>) -> ChatStream {
        parse_sse(stream::iter(
            parts
                .into_iter()
                .map(|part| Ok::<Bytes, reqwest::Error>(Bytes::from_static(part))),
        ))
    }

    async fn drain_tokens(stream: ChatStream) -> Vec<String> {
        let mut stream = stream;
        let mut tokens = Vec::new();
        while let Some(item) = stream.next().await {
            tokens.push(item.expect("stream item"));
        }
        tokens
    }

    #[tokio::test]
    async fn sse_parses_crlf_event_boundaries() {
        let body: &'static [u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        assert_eq!(drain_tokens(chunked(vec![body])).await, vec!["Hello".to_string()]);
    }

    #[tokio::test]
    async fn sse_flushes_final_event_without_blank_line() {
        let body: &'static [u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}";
        assert_eq!(
            drain_tokens(chunked(vec![body])).await,
            vec!["Hello".to_string(), "world".to_string()]
        );
    }

    #[tokio::test]
    async fn sse_buffers_event_split_inside_boundary() {
        let body: &'static [u8] =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n";
        let cut = 47; // between the two '\n' of the first boundary
        let (first, second) = body.split_at(cut);
        assert_eq!(
            drain_tokens(chunked(vec![first, second])).await,
            vec!["Hi".to_string()]
        );
    }
}
