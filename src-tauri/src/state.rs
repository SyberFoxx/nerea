use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use nerea_core::{
    factory::{self, PlatformKeys},
    memory::{plan, Memory, MemoryStore},
    presets,
    transcript::Transcript,
    AiProvider, ChatMessage, ChatRequest, Config, Conversation, Entry, Role,
};
use serde::Serialize;
use tokio::sync::Mutex;

pub const SERVICE: &str = "nerea";

const SYSTEM_PROMPT: &str = "You are Nerea, a concise AI desktop companion living on the user's \
Linux machine. Answer in the user's language. Keep replies short unless the user asks for depth. \
Never invent facts about the user; ask instead. Earlier turns may come from a previous session: \
treat them as your own memory of the conversation.";

/// Resumes from the summary instead of re-sending the whole transcript.
const SUMMARY_PROMPT: &str = "Sos la memoria de Nerea, una compañera de escritorio. Recibís la \
memoria previa y un bloque de turnos nuevos. Devolvés la memoria actualizada, en español neutro, en \
tercera persona, con 350 palabras como máximo. Conservá solo lo que siga siendo útil: quién es el \
usuario, sus proyectos, decisiones tomadas, preferencias, pendientes y hechos del vínculo. No \
inventes nada. No resumas saludos ni charla trivial. Respondé únicamente la memoria.";

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
    memory: Memory,
}

/// Shared state behind the IPC commands.
///
/// Locks are held only for bookkeeping, never across a stream, so `cancel` and
/// `status` stay responsive while a reply is in flight. Cloning is cheap and
/// lets a background task (the summarizer) outlive the command that started it.
#[derive(Clone)]
pub struct AppState {
    session: Arc<Mutex<Session>>,
    cancel: Arc<AtomicBool>,
    transcript: Arc<Transcript>,
    memory_store: Arc<MemoryStore>,
    restored: Arc<AtomicUsize>,
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
    /// Words in the long-term summary.
    pub memory_words: usize,
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
        let transcript = Arc::new(Transcript::open());
        let memory_store = Arc::new(MemoryStore::open());
        let memory = memory_store.load();

        // Only the turns the summary does not already cover go back into the
        // prompt: the rest is condensed in `memory.text`.
        let mut restored: Vec<ChatMessage> = transcript
            .entries()
            .into_iter()
            .filter(|entry| entry.at > memory.through && entry.role != Role::System)
            .map(|entry| entry.message())
            .collect();

        let keep_from = restored.len().saturating_sub(RESTORED_MESSAGES);
        restored.drain(..keep_from);

        let mut conversation = Conversation::new(SYSTEM_PROMPT, CONTEXT_BUDGET_TOKENS);
        for message in restored.iter().cloned() {
            conversation.push(message);
        }

        Self {
            session: Arc::new(Mutex::new(Session {
                conversation,
                provider: None,
                warnings: Vec::new(),
                memory,
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            transcript,
            memory_store,
            restored: Arc::new(AtomicUsize::new(restored.len())),
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
            memory_words: session.memory.words(),
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

        let summary = session.memory.text.clone();
        session.conversation.request_with_memory(Some(&summary))
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
        session.memory = Memory::default();

        if let Err(err) = self.transcript.clear() {
            note(&mut session.warnings, format!("memory clear failed: {err}"));
        }
        if let Err(err) = self.memory_store.clear() {
            note(&mut session.warnings, format!("summary clear failed: {err}"));
        }

        self.restored.store(0, Ordering::Relaxed);
    }

    /// Folds the oldest unsummarized turns into the running summary.
    ///
    /// Returns the new summary when one was written. A failed call leaves the
    /// previous memory untouched, so the next turn simply tries again.
    pub async fn summarize_if_due(&self, provider: &Arc<dyn AiProvider>) -> Option<String> {
        let (previous, through) = {
            let session = self.session.lock().await;
            (session.memory.text.clone(), session.memory.through)
        };

        let pending: Vec<Entry> = self
            .transcript
            .entries()
            .into_iter()
            .filter(|entry| entry.at > through && entry.role != Role::System)
            .collect();

        let take = plan(pending.len())?;
        let chunk = &pending[..take];
        let through_new = chunk.last()?.at;

        let answer = provider.chat_completion(&summary_request(&previous, chunk)).await.ok()?;
        let text = answer.content.trim().to_owned();
        if text.is_empty() {
            return None;
        }

        let memory = Memory { text: text.clone(), through: through_new };
        if let Err(err) = self.memory_store.save(&memory) {
            let mut session = self.session.lock().await;
            note(&mut session.warnings, format!("summary save failed: {err}"));
            return None;
        }

        let mut session = self.session.lock().await;
        session.memory = memory;
        Some(text)
    }

    /// Everything ever stored, for the read-only history window.
    pub fn transcript_entries(&self) -> Vec<Entry> {
        self.transcript.entries()
    }

    pub async fn memory_text(&self) -> String {
        self.session.lock().await.memory.text.clone()
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

fn summary_request(previous: &str, chunk: &[Entry]) -> ChatRequest {
    let mut rendered = String::new();
    for entry in chunk {
        let who = if entry.role == Role::User { "Lioner" } else { "Nerea" };
        rendered.push_str(who);
        rendered.push_str(": ");
        rendered.push_str(entry.content.trim());
        rendered.push('\n');
    }

    let previous = if previous.trim().is_empty() { "(todavía no hay memoria)" } else { previous.trim() };

    ChatRequest {
        messages: vec![
            ChatMessage::system(SUMMARY_PROMPT),
            ChatMessage::user(format!("Memoria previa:\n{previous}\n\nTurnos nuevos:\n{rendered}")),
        ],
        session_id: "memory".to_owned(),
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

    fn entry(role: Role, content: &str, at: u64) -> Entry {
        Entry { role, content: content.to_owned(), at }
    }

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

    #[test]
    fn the_summary_request_names_both_sides_and_carries_the_old_memory() {
        let chunk = vec![
            entry(Role::User, "quiero ordenar el escritorio", 10),
            entry(Role::Assistant, "ara ara, veamos", 11),
        ];

        let request = summary_request("memoria vieja", &chunk);
        assert_eq!(request.session_id, "memory");
        assert_eq!(request.messages.len(), 2);

        let body = &request.messages[1].content;
        assert!(body.contains("memoria vieja"));
        assert!(body.contains("Lioner: quiero ordenar el escritorio"));
        assert!(body.contains("Nerea: ara ara, veamos"));
    }

    #[test]
    fn the_first_summary_says_there_is_none_yet() {
        let request = summary_request("   ", &[entry(Role::User, "hola", 1)]);
        assert!(request.messages[1].content.contains("(todavía no hay memoria)"));
    }
}
