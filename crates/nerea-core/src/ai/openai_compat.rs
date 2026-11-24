use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use reqwest::header::HeaderName;

use super::{
    AiProvider, ChatMessage, ChatRequest, ChatResponse, ChatStream, StreamItem, ToolCall, ToolSpec,
    Usage,
};
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
        let builder = self.client.post(self.endpoint()).bearer_auth(&self.api_key);

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
    messages: Vec<WireMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "<[WireTool]>::is_empty")]
    tools: Vec<WireTool<'a>>,
}

/// One place builds the body, so the wire shape cannot drift between the
/// streaming call and the plain one.
fn chat_body<'a>(model: &'a str, request: &'a ChatRequest, stream: bool) -> ChatBody<'a> {
    ChatBody {
        model,
        messages: wire_messages(&request.messages),
        stream,
        tools: wire_tools(&request.tools),
    }
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<WireCall<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<&'a str>,
}

#[derive(Serialize)]
struct WireCall<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireFunction<'a>,
}

#[derive(Serialize)]
struct WireFunction<'a> {
    name: &'a str,
    arguments: &'a str,
}

#[derive(Serialize)]
struct WireTool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: WireToolFunction<'a>,
}

#[derive(Serialize)]
struct WireToolFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: &'a Value,
}

fn wire_messages(messages: &[ChatMessage]) -> Vec<WireMessage<'_>> {
    messages
        .iter()
        .map(|message| WireMessage {
            role: message.role.as_str(),
            content: message.content.as_str(),
            tool_calls: message.tool_calls.as_ref().map(|calls| {
                calls
                    .iter()
                    .map(|call| WireCall {
                        id: call.id.as_str(),
                        kind: "function",
                        function: WireFunction {
                            name: call.name.as_str(),
                            arguments: call.arguments.as_str(),
                        },
                    })
                    .collect()
            }),
            tool_call_id: message.tool_call_id.as_deref(),
        })
        .collect()
}

fn wire_tools(tools: &[ToolSpec]) -> Vec<WireTool<'_>> {
    tools
        .iter()
        .map(|tool| WireTool {
            kind: "function",
            function: WireToolFunction {
                name: tool.name.as_str(),
                description: tool.description.as_str(),
                parameters: &tool.parameters,
            },
        })
        .collect()
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
    #[serde(default)]
    tool_calls: Vec<WireCallIn>,
}

#[derive(Deserialize)]
struct WireCallIn {
    #[serde(default)]
    id: String,
    #[serde(default)]
    function: Option<WireFunctionIn>,
}

#[derive(Deserialize)]
struct WireFunctionIn {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
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
    #[serde(default)]
    tool_calls: Vec<StreamCall>,
}

#[derive(Deserialize)]
struct StreamCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamCallFunction>,
}

#[derive(Deserialize)]
struct StreamCallFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
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
        let body = chat_body(&self.model, request, false);

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
        let message = message.unwrap_or(ResponseMessage { content: None, tool_calls: Vec::new() });

        Ok(ChatResponse {
            provider: self.id.clone(),
            content: message.content.unwrap_or_default(),
            model: if completion.model.is_empty() {
                self.model.clone()
            } else {
                completion.model
            },
            finish_reason,
            usage: completion.usage,
            tool_calls: message.tool_calls.into_iter().map(into_call).collect(),
        })
    }

    async fn stream_chat_completion(
        &self,
        request: &ChatRequest,
    ) -> Result<ChatStream, ProviderError> {
        let body = chat_body(&self.model, request, true);

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

fn into_call(call: WireCallIn) -> ToolCall {
    let function = call.function.unwrap_or(WireFunctionIn { name: String::new(), arguments: String::new() });
    ToolCall { id: call.id, name: function.name, arguments: function.arguments }
}

/// A call the stream is still assembling: the name may arrive in one chunk and
/// the JSON arguments in twenty.
#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

impl PartialCall {
    fn into_call(self) -> ToolCall {
        ToolCall { id: self.id, name: self.name, arguments: self.arguments }
    }
}

struct StreamState<S> {
    body: std::pin::Pin<Box<S>>,
    buffer: Vec<u8>,
    /// The body is over; only the tail is left to flush.
    exhausted: bool,
    /// `[DONE]` was seen: nothing more will arrive, ever.
    closed: bool,
    /// The calls were already handed to the caller.
    emitted: bool,
    calls: BTreeMap<usize, PartialCall>,
}

impl<S> StreamState<S> {
    fn merge(&mut self, calls: Vec<StreamCall>) {
        for call in calls {
            let partial = self.calls.entry(call.index).or_default();
            if let Some(id) = call.id {
                partial.id = id;
            }
            if let Some(function) = call.function {
                if let Some(name) = function.name {
                    partial.name = name;
                }
                if let Some(arguments) = function.arguments {
                    partial.arguments.push_str(&arguments);
                }
            }
        }
    }

    /// The calls, once, as the last item of the stream.
    fn finish(
        mut self,
    ) -> Option<(Result<StreamItem, ProviderError>, Self)> {
        if self.emitted || self.calls.is_empty() {
            return None;
        }
        self.emitted = true;
        let calls = std::mem::take(&mut self.calls)
            .into_values()
            .map(PartialCall::into_call)
            .collect();
        Some((Ok(StreamItem::calls(calls)), self))
    }
}

fn parse_sse<S>(body: S) -> ChatStream
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    let state = StreamState {
        body: Box::pin(body),
        buffer: Vec::new(),
        exhausted: false,
        closed: false,
        emitted: false,
        calls: BTreeMap::new(),
    };

    Box::pin(stream::unfold(state, |mut state| async move {
        loop {
            if state.closed {
                return state.finish();
            }

            if let Some((end, separator)) = event_boundary(&state.buffer) {
                let event = String::from_utf8_lossy(&state.buffer[..end]).into_owned();
                state.buffer.drain(..end + separator);

                match classify(&event) {
                    SseEvent::Done => {
                        state.closed = true;
                        return state.finish();
                    }
                    SseEvent::Ignore => continue,
                    SseEvent::Chunk(chunk) => {
                        state.merge(chunk.calls);
                        if !chunk.content.is_empty() {
                            return Some((Ok(StreamItem::text(chunk.content)), state));
                        }
                        continue;
                    }
                }
            }

            if state.exhausted {
                // Flush a final event that arrived without its blank line.
                if state.buffer.is_empty() {
                    state.closed = true;
                    return state.finish();
                }
                let event = String::from_utf8_lossy(&state.buffer).into_owned();
                state.buffer.clear();
                match classify(&event) {
                    SseEvent::Chunk(chunk) => {
                        state.merge(chunk.calls);
                        if !chunk.content.is_empty() {
                            return Some((Ok(StreamItem::text(chunk.content)), state));
                        }
                        continue;
                    }
                    SseEvent::Done => {
                        state.closed = true;
                        return state.finish();
                    }
                    SseEvent::Ignore => {
                        state.closed = true;
                        return state.finish();
                    }
                }
            }

            match state.body.as_mut().next().await {
                Some(Ok(chunk)) => state.buffer.extend_from_slice(&chunk),
                Some(Err(err)) => {
                    return Some((Err(ProviderError::Network(err)), state));
                }
                None => state.exhausted = true,
            }
        }
    }))
}

struct Chunk {
    content: String,
    calls: Vec<StreamCall>,
}

enum SseEvent {
    Chunk(Chunk),
    Done,
    Ignore,
}

fn classify(event: &str) -> SseEvent {
    let Some(data) = sse_data(event) else {
        return SseEvent::Ignore;
    };
    if data.trim() == "[DONE]" {
        return SseEvent::Done;
    }

    // A gateway may send keep-alives the spec does not describe; a chunk we
    // cannot read is skipped, never fatal.
    match decode_chunk(&data) {
        Ok(chunk) => SseEvent::Chunk(chunk),
        Err(_) => SseEvent::Ignore,
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

fn decode_chunk(data: &str) -> Result<Chunk, ProviderError> {
    let chunk: StreamChunk = serde_json::from_str(data)?;
    let delta = chunk.choices.into_iter().next().and_then(|choice| choice.delta);

    Ok(match delta {
        Some(delta) => Chunk { content: delta.content.unwrap_or_default(), calls: delta.tool_calls },
        None => Chunk { content: String::new(), calls: Vec::new() },
    })
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
        ChatRequest::new(vec![ChatMessage::user("hi")], "test-session")
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
        assert!(response.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn chat_completion_returns_the_tool_calls_it_asked_for() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"/tmp/x\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        let url = spawn_mock("application/json", vec![body.to_owned()]).await;

        let response = provider(url).chat_completion(&request()).await.expect("call");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "read_file");
        assert_eq!(response.tool_calls[0].id, "call_1");
        assert_eq!(response.tool_calls[0].arguments, r#"{"path":"/tmp/x"}"#);
        assert_eq!(response.finish_reason.as_deref(), Some("tool_calls"));
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
            let item = item.expect("chunk");
            collected.push_str(&item.content);
        }
        assert_eq!(collected, "Hello, world");
    }

    /// The arguments of a call arrive one fragment at a time; the item that
    /// comes out has to hold the whole JSON.
    #[tokio::test]
    async fn stream_assembles_a_tool_call_split_across_chunks() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_9\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"/tmp/a.txt\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let url = spawn_mock("text/event-stream", vec![body.to_owned()]).await;

        let mut stream = provider(url)
            .stream_chat_completion(&request())
            .await
            .expect("stream");

        let mut calls = Vec::new();
        while let Some(item) = stream.next().await {
            let item = item.expect("chunk");
            calls.extend(item.tool_calls);
        }

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_9");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, r#"{"path":"/tmp/a.txt"}"#);
    }

    #[tokio::test]
    async fn a_stream_of_plain_text_ends_without_calls() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"listo\"}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let url = spawn_mock("text/event-stream", vec![body.to_owned()]).await;

        let mut stream = provider(url)
            .stream_chat_completion(&request())
            .await
            .expect("stream");

        let mut text = String::new();
        let mut calls = 0;
        while let Some(item) = stream.next().await {
            let item = item.expect("chunk");
            text.push_str(&item.content);
            calls += item.tool_calls.len();
        }

        assert_eq!(text, "listo");
        assert_eq!(calls, 0);
    }

    #[test]
    fn tools_travel_in_the_shape_the_api_expects() {
        let tools = vec![ToolSpec {
            name: "read_file".to_owned(),
            description: "Read a file".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
        }];
        let wire = serde_json::to_value(wire_tools(&tools)).expect("serialize");

        assert_eq!(wire[0]["type"], "function");
        assert_eq!(wire[0]["function"]["name"], "read_file");
        assert_eq!(wire[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn a_tool_result_carries_the_call_it_answers() {
        let messages = vec![
            ChatMessage::assistant_tools("", vec![ToolCall {
                id: "call_1".to_owned(),
                name: "read_file".to_owned(),
                arguments: "{}".to_owned(),
            }]),
            ChatMessage::tool_result("call_1", "contenido"),
        ];
        let wire = serde_json::to_value(wire_messages(&messages)).expect("serialize");

        assert_eq!(wire[0]["role"], "assistant");
        assert_eq!(wire[0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(wire[0]["tool_calls"][0]["function"]["name"], "read_file");
        assert_eq!(wire[1]["role"], "tool");
        assert_eq!(wire[1]["tool_call_id"], "call_1");
        assert_eq!(wire[1]["content"], "contenido");
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
            .chat_completion(&ChatRequest::new(vec![ChatMessage::user("hi")], "sess-42"))
            .await
            .expect("gateway accepted the request");

        assert_eq!(response.content, "ok");
    }

    #[tokio::test]
    async fn does_not_leak_the_session_id_to_providers_that_ignore_it() {
        let url = spawn_header_guard("sess-42", false).await;

        // DeepSeek-style: no session header configured, so it must not be sent.
        let response = provider(url)
            .chat_completion(&ChatRequest::new(vec![ChatMessage::user("hi")], "sess-42"))
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

    async fn drain_content(stream: ChatStream) -> String {
        let mut stream = stream;
        let mut text = String::new();
        while let Some(item) = stream.next().await {
            text.push_str(&item.expect("stream item").content);
        }
        text
    }

    #[tokio::test]
    async fn sse_parses_crlf_event_boundaries() {
        let body: &'static [u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\r\n\r\ndata: [DONE]\r\n\r\n";
        assert_eq!(drain_content(chunked(vec![body])).await, "Hello");
    }

    #[tokio::test]
    async fn sse_flushes_final_event_without_blank_line() {
        let body: &'static [u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}";
        assert_eq!(drain_content(chunked(vec![body])).await, "Helloworld");
    }

    #[tokio::test]
    async fn sse_buffers_event_split_inside_boundary() {
        let body: &'static [u8] =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n";
        let cut = 47; // between the two '\n' of the first boundary
        let (first, second) = body.split_at(cut);
        assert_eq!(drain_content(chunked(vec![first, second])).await, "Hi");
    }
}
