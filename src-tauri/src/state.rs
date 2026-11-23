use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use nerea_core::{
    factory::{self, PlatformKeys},
    presets, AiProvider, ChatRequest, Config, Conversation,
};
use serde::Serialize;
use tokio::sync::Mutex;

pub const SERVICE: &str = "nerea";

const SYSTEM_PROMPT: &str = "You are Nerea, a concise AI desktop companion living on the user's \
Linux machine. Answer in the user's language. Keep replies short unless the user asks for depth. \
Never invent facts about the user; ask instead.";

/// Same budget as the CLI: recent context, not a full window.
const CONTEXT_BUDGET_TOKENS: usize = 8_000;

struct Session {
    conversation: Conversation,
    provider: Option<Arc<dyn AiProvider>>,
    warnings: Vec<String>,
}

/// Shared state behind the IPC commands.
///
/// Locks are held only for bookkeeping, never across a stream, so `cancel` and
/// `status` stay responsive while a reply is in flight.
pub struct AppState {
    session: Mutex<Session>,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub provider: String,
    pub model: Option<String>,
    pub turns: usize,
    pub session_id: String,
    pub warnings: Vec<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(Session {
                conversation: Conversation::new(SYSTEM_PROMPT, CONTEXT_BUDGET_TOKENS),
                provider: None,
                warnings: Vec::new(),
            }),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Builds the provider chain on first use; a missing key surfaces here.
    pub async fn ensure_provider(&self) -> Result<Arc<dyn AiProvider>, String> {
        let mut session = self.session.lock().await;
        if let Some(provider) = &session.provider {
            return Ok(provider.clone());
        }

        let config = Config::load().map_err(|err| err.to_string())?;
        let keys = PlatformKeys::new(SERVICE);
        let chain = factory::build_chain(&config, &keys, true).map_err(|err| err.to_string())?;

        session.warnings = chain.warnings;
        session.provider = Some(chain.provider.clone());

        Ok(chain.provider)
    }

    pub async fn status(&self) -> Status {
        let session = self.session.lock().await;

        let (provider, model) = match session.provider.as_ref() {
            Some(provider) => (provider.id().to_owned(), Some(provider.model().to_owned())),
            None => {
                let (id, model) = configured();
                let model =
                    model.or_else(|| presets::find(&id).map(|preset| preset.default_model.to_owned()));
                (id, model)
            }
        };

        Status {
            provider,
            model,
            turns: session.conversation.len(),
            session_id: session.conversation.session_id().to_owned(),
            warnings: session.warnings.clone(),
        }
    }

    pub async fn begin_turn(&self, text: &str) -> ChatRequest {
        self.cancel.store(false, Ordering::SeqCst);
        let mut session = self.session.lock().await;
        session.conversation.push_user(text);
        session.conversation.request()
    }

    /// Returns the new turn count. An empty reply (cancelled or failed early) is
    /// not written into history.
    pub async fn finish_turn(&self, reply: &str) -> usize {
        let mut session = self.session.lock().await;
        if !reply.is_empty() {
            session.conversation.push_assistant(reply);
        }
        session.conversation.len()
    }

    pub async fn reset(&self) {
        let mut session = self.session.lock().await;
        session.conversation = Conversation::new(SYSTEM_PROMPT, CONTEXT_BUDGET_TOKENS);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn take_cancel(&self) -> bool {
        self.cancel.swap(false, Ordering::SeqCst)
    }
}

fn configured() -> (String, Option<String>) {
    match Config::load() {
        Ok(config) => (config.provider, config.model),
        Err(_) => (Config::default().provider, None),
    }
}
