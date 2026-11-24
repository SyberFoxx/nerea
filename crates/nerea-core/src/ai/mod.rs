//! Provider contract.
//!
//! Providers are interchangeable: one `dyn AiProvider`, one normalized shape at
//! the boundary, one wire format per module. Tools are part of that contract —
//! the API says when a model wants one, so no prompt ever has to.

pub mod fallback;
pub mod openai_compat;
pub mod presets;
pub mod registry;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

pub type ChatStream = Pin<Box<dyn Stream<Item = Result<StreamItem, ProviderError>> + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Set on the assistant turn that asked for tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Set on the turn that carries one tool's result back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    fn plain(role: Role, content: impl Into<String>) -> Self {
        Self { role, content: content.into(), tool_calls: None, tool_call_id: None }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::plain(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::plain(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::plain(Role::Assistant, content)
    }

    /// The assistant turn that asked for tools, with whatever she said in the
    /// same breath. Dropping that text makes her forget her own sentence: the
    /// next round reads as if she went silent and only called tools.
    pub fn assistant_tools(content: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Some(calls),
            tool_call_id: None,
        }
    }

    /// One tool's result, bound to the call it answers.
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(call_id.into()),
        }
    }
}

/// One call the model asked for. `arguments` is the JSON it wrote, untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// What a tool looks like to the model: name, purpose and a JSON Schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// One piece of a streamed answer: text, or the calls it decided on.
#[derive(Debug, Clone, Default)]
pub struct StreamItem {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

impl StreamItem {
    pub fn is_empty(&self) -> bool {
        self.content.is_empty() && self.tool_calls.is_empty()
    }

    pub fn text(content: impl Into<String>) -> Self {
        Self { content: content.into(), tool_calls: Vec::new() }
    }

    pub fn calls(tool_calls: Vec<ToolCall>) -> Self {
        Self { content: String::new(), tool_calls }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, alias = "input_tokens")]
    pub prompt_tokens: u32,
    #[serde(default, alias = "output_tokens")]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

/// Normalized shape: what the app consumes, never raw provider JSON.
/// `Serialize` because it crosses the IPC boundary as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    /// Which provider produced this answer; makes failover observable.
    pub provider: String,
    pub content: String,
    pub model: String,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

/// One provider call: the trimmed history plus the conversation it belongs to.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    /// Stable per-conversation id. Some gateways use it for routing and prompt
    /// caching (OpenCode Go expects it in `x-opencode-session`).
    pub session_id: String,
    /// Empty when the caller wants no tools at all.
    pub tools: Vec<ToolSpec>,
}

impl ChatRequest {
    pub fn new(messages: Vec<ChatMessage>, session_id: impl Into<String>) -> Self {
        Self { messages, session_id: session_id.into(), tools: Vec::new() }
    }

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }
}

#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Stable id: "opencode-go", "deepseek", ...
    fn id(&self) -> &str;

    /// Model this instance is bound to.
    fn model(&self) -> &str;

    async fn chat_completion(&self, request: &ChatRequest) -> Result<ChatResponse, ProviderError>;

    async fn stream_chat_completion(
        &self,
        request: &ChatRequest,
    ) -> Result<ChatStream, ProviderError>;
}
