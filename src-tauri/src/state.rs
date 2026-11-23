use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use nerea_core::{
    factory::{self, PlatformKeys},
    presets,
    transcript::Transcript,
    AiProvider, ChatRequest, Config, Conversation, Entry, Role,
};
use serde::Serialize;
use tokio::sync::Mutex;

pub const SERVICE: &str = "nerea";

const SYSTEM_PROMPT: &str = "You are Nerea, a concise AI desktop companion living on the user's \
Linux machine. Answer in the user's language. Keep replies short unless the user asks for depth. \
Never invent facts about the user; ask instead. Earlier turns may come from a previous session: \
treat them as your own memory of the conversation.";

/// Same budget as the CLI: recent context, not a full window.
const CONTEXT_BUDGET_TOKENS: usize = 8_000;

/// How much of the stored conversation is restored into the prompt.
const RESTORED_MESSAGES: usize = 400;

/// Warnings are informative, not a log: more than a handful means noise.
const MAX_WARNINGS: usize = 6;

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
    transcript: Transcript,
    restored: AtomicUsize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub provider: String,
    pub model: Option<String>,
    pub turns: usize,
    /// Estimated tokens held in memory, system prompt excluded.
    pub tokens: usize,
    /// Turns loaded from disk when the app started.
    pub restored: usize,
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
        let transcript = Transcript::open();
        let restored = transcript.recent(RESTORED_MESSAGES);

        let mut conversation = Conversation::new(SYSTEM_PROMPT, CONTEXT_BUDGET_TOKENS);
        for message in restored.iter().cloned() {
            conversation.push(message);
        }

        Self {
            session: Mutex::new(Session {
                conversation,
                provider: None,
                warnings: Vec::new(),
            }),
            cancel: Arc::new(AtomicBool::new(false)),
            transcript,
            restored: AtomicUsize::new(restored.len()),
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
            tokens: session.conversation.estimated_tokens(),
            restored: self.restored.load(Ordering::Relaxed),
            session_id: session.conversation.session_id().to_owned(),
            warnings: session.warnings.clone(),
        }
    }

    pub async fn begin_turn(&self, text: &str) -> ChatRequest {
        self.cancel.store(false, Ordering::SeqCst);
        let mut session = self.session.lock().await;
        session.conversation.push_user(text);

        if let Err(err) = self.transcript.append(Role::User, text) {
            note(&mut session.warnings, format!("memory write failed: {err}"));
        }

        session.conversation.request()
    }

    /// Returns the new turn count. An empty reply (cancelled or failed early) is
    /// not written into history.
    pub async fn finish_turn(&self, reply: &str) -> usize {
        let mut session = self.session.lock().await;
        if !reply.is_empty() {
            session.conversation.push_assistant(reply);
            if let Err(err) = self.transcript.append(Role::Assistant, reply) {
                note(&mut session.warnings, format!("memory write failed: {err}"));
            }
        }
        session.conversation.len()
    }

    pub async fn reset(&self) {
        let mut session = self.session.lock().await;
        session.conversation = Conversation::new(SYSTEM_PROMPT, CONTEXT_BUDGET_TOKENS);
        if let Err(err) = self.transcript.clear() {
            note(&mut session.warnings, format!("memory clear failed: {err}"));
        }
        self.restored.store(0, Ordering::Relaxed);
    }

    /// Everything ever stored, for the read-only history window.
    pub fn transcript_entries(&self) -> Vec<Entry> {
        self.transcript.entries()
    }

    /// True once any earlier session left something behind.
    pub fn has_memory(&self) -> bool {
        self.restored.load(Ordering::Relaxed) > 0
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn take_cancel(&self) -> bool {
        self.cancel.swap(false, Ordering::SeqCst)
    }
}

/// Keeps warnings short and free of duplicates.
fn note(warnings: &mut Vec<String>, warning: String) {
    if warnings.iter().any(|existing| existing == &warning) {
        return;
    }
    if warnings.len() >= MAX_WARNINGS {
        return;
    }
    warnings.push(warning);
}

fn configured() -> (String, Option<String>) {
    match Config::load() {
        Ok(config) => (config.provider, config.model),
        Err(_) => (Config::default().provider, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_are_deduplicated_and_bounded() {
        let mut warnings = Vec::new();
        note(&mut warnings, "one".to_owned());
        note(&mut warnings, "one".to_owned());
        assert_eq!(warnings.len(), 1);

        for index in 0..20 {
            note(&mut warnings, format!("w{index}"));
        }
        assert_eq!(warnings.len(), MAX_WARNINGS);
    }
}
