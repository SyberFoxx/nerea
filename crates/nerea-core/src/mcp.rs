//! A small MCP client, and the one tool that uses it.
//!
//! Context7 is a remote MCP server: documentation for libraries, kept current.
//! It exposes two tools with large schemas, and every schema is paid on **every
//! request** — with an 8 000-token window that is a tax for something she uses
//! now and then. So the protocol is here, and the model only sees `docs`.
//!
//! Transport: MCP over streamable HTTP. A request is a JSON-RPC POST, and the
//! answer arrives either as `application/json` or as a `text/event-stream`,
//! which is why the body is parsed twice instead of assumed.

use std::time::Duration;

use serde_json::{Value, json};

/// The public server. No key: it is rate-limited, which is fine for a companion.
pub const CONTEXT7: &str = "https://mcp.context7.com/mcp";
const PROTOCOL: &str = "2025-06-18";

pub struct Client {
    url: String,
    http: reqwest::Client,
    session: Option<String>,
}

impl Client {
    pub fn new(url: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(25))
            .build()
            .map_err(|err| err.to_string())?;
        Ok(Self { url: url.to_owned(), http, session: None })
    }

    pub fn context7() -> Result<Self, String> {
        let url = std::env::var("NEREA_MCP_URL").unwrap_or_else(|_| CONTEXT7.to_owned());
        Self::new(&url)
    }

    /// The handshake MCP requires before any call.
    pub async fn open(&mut self) -> Result<(), String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL,
                "capabilities": {},
                "clientInfo": { "name": "nerea", "version": env!("CARGO_PKG_VERSION") }
            }
        });

        let response = self.post(body).await?;
        if let Some(session) = response.session {
            self.session = Some(session);
        }

        // Best effort: some servers ignore it, none require an answer.
        let _ = self
            .post(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;

        Ok(())
    }

    /// Calls one tool and returns its text.
    pub async fn call(&mut self, tool: &str, arguments: Value) -> Result<String, String> {
        if self.session.is_none() {
            self.open().await?;
        }

        let body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments }
        });

        let response = self.post(body).await;
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                // The session may have expired under us: one fresh handshake.
                self.session = None;
                self.open().await?;
                self.post(json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": { "name": tool, "arguments": arguments }
                }))
                .await?
            }
        };

        if let Some(error) = response.payload.get("error") {
            let message = error.get("message").and_then(Value::as_str).unwrap_or("error");
            return Err(format!("el servidor MCP respondió: {message}"));
        }

        Ok(text_of(&response.payload))
    }

    async fn post(&self, body: Value) -> Result<Answer, String> {
        let mut request = self
            .http
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .json(&body);

        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }

        let response = request.send().await.map_err(|err| format!("MCP: {err}"))?;
        let status = response.status();
        let session = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let kind = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let text = response.text().await.map_err(|err| err.to_string())?;

        if !status.is_success() {
            return Err(format!("MCP respondió {status}: {}", first_chars(&text, 200)));
        }

        let payload = if kind.contains("event-stream") {
            last_event_payload(&text)
        } else {
            serde_json::from_str::<Value>(&text).ok()
        };

        let payload = payload.ok_or_else(|| {
            format!("MCP devolvió algo ilegible: {}", first_chars(&text, 200))
        })?;

        Ok(Answer { payload, session })
    }
}

struct Answer {
    payload: Value,
    session: Option<String>,
}

/// In an event stream the answer is the payload of the last `data:` line, which
/// is where MCP puts the JSON-RPC response.
pub fn last_event_payload(text: &str) -> Option<Value> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|payload| serde_json::from_str::<Value>(payload.trim()).ok())
        .next_back()
}

/// The text of a `tools/call` result: a list of content items.
pub fn text_of(payload: &Value) -> String {
    let Some(items) = payload.pointer("/result/content").and_then(Value::as_array) else {
        return first_chars(&payload.to_string(), 4_000);
    };

    let mut out = String::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                out.push_str(item.get("text").and_then(Value::as_str).unwrap_or_default());
                out.push('\n');
            }
            _ => {
                out.push_str(&format!("[{}]\n", item.get("type").and_then(Value::as_str).unwrap_or("otro")));
            }
        }
    }

    let trimmed = out.trim().to_owned();
    if trimmed.is_empty() {
        "El servidor no devolvió texto.".to_owned()
    } else {
        trimmed
    }
}

fn first_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    format!("{}…", text.chars().take(max).collect::<String>())
}

/// Documentation for one library and one question.
///
/// Two MCP calls: the id first when he gave a name and not a path, then the
/// docs. Context7 wants `/org/project` and a bare name is not that.
pub async fn docs(library: &str, question: &str) -> Result<String, String> {
    let library = library.trim();
    let question = question.trim();
    if library.is_empty() || question.is_empty() {
        return Err("necesito la librería y la pregunta".to_owned());
    }

    let mut client = Client::context7()?;

    let id = if library.starts_with('/') {
        library.to_owned()
    } else {
        let answer = client
            .call(
                "resolve-library-id",
                json!({ "query": question, "libraryName": library }),
            )
            .await?;
        extract_id(&answer).ok_or_else(|| {
            format!("no encontré «{library}» en context7; probá con el nombre del paquete")
        })?
    };

    client.call("query-docs", json!({ "libraryId": id, "query": question })).await
}

/// Context7 answers with a list of candidates; the first id is the one it ranks
/// highest. `/org/project` is the shape of that id.
pub fn extract_id(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|word| word.trim_matches(|character: char| {
            character == '"' || character == '\'' || character == ',' || character == '('
                || character == ')' || character == '`'
        }))
        .find(|word| word.starts_with('/') && word.matches('/').count() >= 2)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_body_is_json() {
        let body = r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"hola"}]}}"#;
        let value: Value = serde_json::from_str(body).expect("json");
        assert_eq!(text_of(&value), "hola");
    }

    #[test]
    fn an_event_stream_takes_its_last_payload() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"primero\"}]}}\n\n\
event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"segundo\"}]}}\n\n";
        let payload = last_event_payload(body).expect("payload");
        assert_eq!(text_of(&payload), "segundo");
    }

    #[test]
    fn the_library_id_is_the_first_path_shaped_word() {
        let answer = "Available libraries:\n- /facebook/react (v19.0.0) — The library\n- /vercel/next.js\n";
        assert_eq!(extract_id(answer).as_deref(), Some("/facebook/react"));
    }

    #[test]
    fn a_plain_library_has_no_id() {
        assert!(extract_id("no encontré nada con ese nombre").is_none());
    }

    #[test]
    fn a_result_without_content_is_not_silent() {
        let value: Value = serde_json::from_str(r#"{"result":{}}"#).expect("json");
        assert!(!text_of(&value).is_empty());
    }
}
