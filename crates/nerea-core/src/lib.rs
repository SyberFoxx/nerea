//! Nerea core: AI providers, secrets, configuration and the conversation contract.
//!
//! This crate knows nothing about windows, UI toolkits or IPC on purpose.

pub mod ai;
pub mod config;
pub mod conversation;
pub mod error;
pub mod factory;
pub mod phrase;
pub mod secrets;
pub mod transcript;

pub use ai::fallback::FallbackProvider;
pub use ai::openai_compat::OpenAiCompatProvider;
pub use ai::presets::{self, ProviderPreset};
pub use ai::registry::ProviderRegistry;
pub use ai::{AiProvider, ChatMessage, ChatRequest, ChatResponse, ChatStream, Role, Usage};
pub use config::{Config, Fallback};
pub use factory::{BuildError, Chain, KeySource, KeyState, PlatformKeys};
pub use phrase::{Phrase, PhraseBank};
pub use conversation::{Conversation, estimate_tokens};
pub use error::ProviderError;
pub use secrets::{SecretError, SecretStore};
pub use transcript::{Entry, Transcript};
