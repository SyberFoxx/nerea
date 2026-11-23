//! Nerea core: AI providers, secrets and the normalized conversation contract.
//!
//! This crate knows nothing about windows, UI toolkits or IPC on purpose.

pub mod ai;
pub mod error;
pub mod secrets;

pub use ai::openai_compat::OpenAiCompatProvider;
pub use ai::presets::{self, ProviderPreset};
pub use ai::registry::ProviderRegistry;
pub use ai::{AiProvider, ChatMessage, ChatResponse, ChatStream, Role};
pub use error::ProviderError;
pub use secrets::{SecretError, SecretStore};
