pub mod fallback;
pub mod openai_compat;
pub mod presets;
pub mod registry;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

/// Boxed so the trait stays `dyn`-compatible and the stream owns its connection.
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<String, ProviderError>> + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into() }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into() }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: content.into() }
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
}

#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Stable id: "opencode-go", "deepseek", ...
    fn id(&self) -> &str;

    /// Model this instance is bound to.
    fn model(&self) -> &str;

    async fn chat_completion(
        &self,
        messages: &[ChatMessage],
    ) -> Result<ChatResponse, ProviderError>;

    async fn stream_chat_completion(
        &self,
        messages: &[ChatMessage],
    ) -> Result<ChatStream, ProviderError>;
}
