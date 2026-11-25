use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};

use nerea_core::{
    cards::{self, Card},
    factory::{self, PlatformKeys},
    memory::{plan, Memory, MemoryStore},
    presets,
    recall::{age_label, Corpus, Hit},
    transcript::Transcript,
    AiProvider, ChatMessage, ChatRequest, Config, Conversation, Entry, PromptContext, Role,
    ToolCall,
};
use serde::Serialize;
use tokio::sync::Mutex;

pub const SERVICE: &str = "nerea";

/// Her self. What she can *do* is declared to the API as tools, not written
/// into her identity: the model is told, it does not have to remember a format.
fn system_prompt() -> String {
    nerea_core::persona::system_prompt()
}

/// Resumes from the summary instead of re-sending the whole transcript.
const SUMMARY_PROMPT: &str = "Sos la memoria de Nerea, una compañera de escritorio. Recibís la \
memoria previa y un bloque de turnos nuevos. Devolvés la memoria actualizada, en español neutro, en \
tercera persona, con 350 palabras como máximo. Conservá solo lo que siga siendo útil: quién es el \
usuario, sus proyectos, decisiones tomadas, preferencias, pendientes y hechos del vínculo. No \
inventes nada. No resumas saludos ni charla trivial. Respondé únicamente la memoria.";

/// Same budget as the CLI: recent context, not a full window.
const CONTEXT_BUDGET_TOKENS: usize = 8_000;
/// How much of an older observation survives once a newer round lands.
const COMPACT_OBSERVATION_CHARS: usize = 400;

/// How much of the stored conversation is restored into the prompt.
const RESTORED_MESSAGES: usize = 400;

/// Warnings are informative, not a log: more than a handful means noise.
const MAX_WARNINGS: usize = 6;

/// How many old episodes a single turn may carry, and how relevant one has to
/// be before it is worth the tokens.
const RECALL_LIMIT: usize = 4;
const RECALL_MIN_SCORE: f64 = 1.0;
/// Turns already in the prompt do not need to be recalled.
const RECALL_SKIP_NEWEST: usize = 12;
/// Rebuild the index once this many turns were added to it.
const RECALL_REBUILD_EVERY: usize = 8;

/// The searchable transcript plus its index.
struct RecallIndex {
    entries: Vec<Entry>,
    corpus: Corpus,
}

impl RecallIndex {
    fn build(entries: Vec<Entry>) -> Self {
        let corpus = Corpus::build(&entries);
        Self { entries, corpus }
    }
}

/// What one turn needs: the prompt, plus how much came out of the past.
pub struct Turn {
    pub request: ChatRequest,
    pub recalled: usize,
    pub oldest_days: u64,
}

struct Session {
    conversation: Conversation,
    provider: Option<Arc<dyn AiProvider>>,
    warnings: Vec<String>,
    memory: Memory,
    recall: RecallIndex,
    since_index: usize,
    /// The knowledge cards, read once at boot and re-read after a `remember`.
    cards: Vec<Card>,
    /// The card block chosen for the turn in flight. Stable across its tool
    /// rounds on purpose: the question did not change between them.
    cards_for_turn: Option<String>,
}

/// Shared state behind the IPC commands.
///
/// Locks are held only for bookkeeping, never across a stream, so `cancel` and
/// `status` stay responsive while a reply is in flight. Cloning is cheap and
/// lets a background task (the summarizer) outlive the command that started it.
#[derive(Clone)]
pub struct AppState {
    session: Arc<Mutex<Session>>,
    /// Commands waiting for his click, by id.
    approvals: Arc<std::sync::Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<bool>>>>,
    approval_seq: Arc<std::sync::atomic::AtomicU64>,
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

        let mut conversation = Conversation::new(system_prompt(), CONTEXT_BUDGET_TOKENS);
        for message in restored.iter().cloned() {
            conversation.push(message);
        }

        let recall = RecallIndex::build(transcript.entries());

        // Cards are read once here: a handful of 2 KB files, not a scan per turn.
        let cards = cards::load(&cards::dir());

        Self {
            session: Arc::new(Mutex::new(Session {
                conversation,
                provider: None,
                warnings: Vec::new(),
                memory,
                recall,
                since_index: 0,
                cards,
                cards_for_turn: None,
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            approvals: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            approval_seq: Arc::new(std::sync::atomic::AtomicU64::new(1)),
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

    /// Registers a command waiting for approval and returns the id and the
    /// channel the answer will arrive on.
    pub fn request_approval(&self) -> (u64, tokio::sync::oneshot::Receiver<bool>) {
        let id = self.approval_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        if let Ok(mut map) = self.approvals.lock() {
            map.insert(id, sender);
        }
        (id, receiver)
    }

    /// The click. A second answer for the same id is ignored.
    pub fn decide(&self, id: u64, approved: bool) -> bool {
        let sender = self.approvals.lock().ok().and_then(|mut map| map.remove(&id));
        match sender {
            Some(sender) => sender.send(approved).is_ok(),
            None => false,
        }
    }

    pub async fn begin_turn(&self, text: &str) -> Turn {
        self.cancel.store(false, Ordering::SeqCst);
        let mut session = self.session.lock().await;
        session.conversation.push_user(text);

        if let Err(err) = self.transcript.append(Role::User, text) {
            note(&mut session.warnings, format!("memory write failed: {err}"));
        }

        let (rendered, recalled, oldest_days) = render_recall(&session.recall, text);

        // The cards are chosen from the message itself. Nothing above the floor
        // means nothing injected: the common turn costs zero tokens.
        let selected = cards::select(&session.cards, text, cards::MAX_SELECTED);
        session.cards_for_turn = cards::block(&selected);

        let summary = session.memory.text.clone();
        let request = session
            .conversation
            .request_with_context(PromptContext {
                memory: Some(&summary),
                cards: session.cards_for_turn.as_deref(),
                recall: rendered.as_deref(),
            })
            .with_tools(nerea_core::tools::specs());

        Turn { request, recalled, oldest_days }
    }

    /// The prompt after a tool exchange has been folded into the live thread.
    pub async fn tool_request(&self) -> ChatRequest {
        let session = self.session.lock().await;
        session
            .conversation
            .request_with_context(PromptContext {
                memory: Some(&session.memory.text),
                cards: session.cards_for_turn.as_deref(),
                recall: None,
            })
            .with_tools(nerea_core::tools::specs())
    }

    /// Keeps what she read in the live thread, never in the transcript: her
    /// memory is a conversation, not a filesystem.
    pub async fn push_tool_round(
        &self,
        said: &str,
        calls: Vec<ToolCall>,
        results: Vec<String>,
    ) {
        let mut session = self.session.lock().await;
        // A card written mid-turn is memory for the next one, not for the round
        // that is answering: re-reading here keeps the turn's prompt stable.
        let wrote = calls.iter().any(|call| call.name == "remember");
        // What she read in an earlier round only needs its opening lines: the
        // question she is answering is worth more than the tail of an old file.
        session.conversation.compact_observations(COMPACT_OBSERVATION_CHARS);
        let ids: Vec<String> = calls.iter().map(|call| call.id.clone()).collect();
        session.conversation.push_tool_round(said, calls);
        for (id, result) in ids.iter().zip(results) {
            session.conversation.push_tool_result(id, result);
        }
        if wrote {
            session.cards = cards::load(&cards::dir());
        }
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
            self.refresh_recall(&mut session);
        }
        session.conversation.len()
    }

    /// Rebuilds the recall index once enough new turns piled up.
    fn refresh_recall(&self, session: &mut Session) {
        session.since_index += 1;
        if session.since_index < RECALL_REBUILD_EVERY {
            return;
        }

        session.since_index = 0;
        session.recall = RecallIndex::build(self.transcript.entries());
    }

    pub async fn reset(&self) {
        let mut session = self.session.lock().await;
        session.conversation = Conversation::new(system_prompt(), CONTEXT_BUDGET_TOKENS);
        session.memory = Memory::default();
        session.recall = RecallIndex::build(Vec::new());
        session.since_index = 0;

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

    /// Read-only peek at what recall would send, for diagnostics.
    pub async fn recall_preview(&self, query: &str) -> Option<String> {
        let session = self.session.lock().await;
        render_recall(&session.recall, query).0
    }

    /// Read-only peek at the card block a message would carry, for diagnostics.
    pub async fn cards_preview(&self, query: &str) -> Option<String> {
        let session = self.session.lock().await;
        let selected = cards::select(&session.cards, query, cards::MAX_SELECTED);
        cards::block(&selected)
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

/// Turns old episodes into the block the prompt carries.
///
/// Returns the rendered block, how many episodes made it, and the age in days
/// of the oldest one, which is what the UI uses to say "me acordé de algo".
fn render_recall(index: &RecallIndex, query: &str) -> (Option<String>, usize, u64) {
    if index.corpus.is_empty() {
        return (None, 0, 0);
    }

    let total = index.entries.len();
    let skip_from = total.saturating_sub(RECALL_SKIP_NEWEST);

    let hits: Vec<Hit> = index
        .corpus
        .search(query, RECALL_LIMIT * 2)
        .into_iter()
        .filter(|hit| hit.score >= RECALL_MIN_SCORE && hit.position < skip_from)
        .take(RECALL_LIMIT)
        .collect();

    if hits.is_empty() {
        return (None, 0, 0);
    }

    let now = now_ms();
    let mut block = String::new();
    let mut used: Vec<usize> = Vec::new();
    let mut oldest = u64::MAX;

    for hit in &hits {
        // A turn is a question and its answer: recalling half of it reads wrong.
        let (start, end) = if index.entries[hit.position].role == Role::User {
            let answer = index
                .entries
                .get(hit.position + 1)
                .is_some_and(|entry| entry.role == Role::Assistant);
            (hit.position, if answer { hit.position + 1 } else { hit.position })
        } else {
            let question = hit.position > 0 && index.entries[hit.position - 1].role == Role::User;
            (if question { hit.position - 1 } else { hit.position }, hit.position)
        };

        if used.contains(&start) || start >= skip_from {
            continue;
        }
        used.push(start);

        for entry in &index.entries[start..=end] {
            if entry.role == Role::System {
                continue;
            }

            oldest = oldest.min(entry.at);

            let who = if entry.role == Role::User { "Lioner" } else { "Nerea" };
            block.push('[');
            block.push_str(&age_label(entry.at, now));
            block.push_str("] ");
            block.push_str(who);
            block.push_str(": ");
            block.push_str(entry.content.trim().chars().take(400).collect::<String>().as_str());
            block.push('\n');
        }
    }

    if block.is_empty() {
        return (None, 0, 0);
    }

    let days = (now.saturating_sub(oldest)) / 86_400_000;
    (Some(block), used.len(), days)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
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

    ChatRequest::new(
        vec![
            ChatMessage::system(SUMMARY_PROMPT),
            ChatMessage::user(format!("Memoria previa:\n{previous}\n\nTurnos nuevos:\n{rendered}")),
        ],
        "memory",
    )
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
    fn recall_pairs_the_answer_with_its_question() {
        let now = now_ms();
        let day = 86_400_000u64;
        let mut entries = vec![
            entry(Role::User, "¿Dónde guardamos las claves de la API?", now - 40 * day),
            entry(Role::Assistant, "En el keyring del sistema.", now - 40 * day),
        ];
        for index in 0..RECALL_SKIP_NEWEST {
            entries.push(entry(Role::User, &format!("charla reciente {index}"), now));
            entries.push(entry(Role::Assistant, "sí", now));
        }

        let index = RecallIndex::build(entries);
        let (block, hits, days) = render_recall(&index, "claves de la api");

        let block = block.expect("a recall block");
        assert_eq!(hits, 1);
        assert_eq!(days, 40);
        assert!(block.contains("Lioner: ¿Dónde guardamos las claves"));
        assert!(block.contains("Nerea: En el keyring"), "the answer comes with it");
        assert!(block.starts_with("[hace 1 mes]"));
    }

    #[test]
    fn the_first_summary_says_there_is_none_yet() {
        let request = summary_request("   ", &[entry(Role::User, "hola", 1)]);
        assert!(request.messages[1].content.contains("(todavía no hay memoria)"));
    }
}
