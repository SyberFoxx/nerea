use std::time::{SystemTime, UNIX_EPOCH};

use crate::ai::{ChatMessage, ChatRequest, Role, ToolCall};

/// Rough characters-per-token ratio. Tokenizers differ per model, so this is a
/// cheap lower bound with margin instead of a per-provider dependency.
const CHARS_PER_TOKEN: usize = 4;

/// Deliberately approximate: the budget is a safety net, not an accounting system.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// What travels with the prompt besides the turns themselves.
///
/// Three blocks with separate budgets: identity always, the summary always, the
/// cards only when the message asks for one. Three positional `Option<&str>`
/// arguments would let a caller pass the summary where the cards belong, and
/// nothing would catch it; a struct cannot.
#[derive(Debug, Default, Clone, Copy)]
pub struct PromptContext<'a> {
    pub memory: Option<&'a str>,
    pub recall: Option<&'a str>,
    pub cards: Option<&'a str>,
}

/// Turn history with a bounded prompt window.
pub struct Conversation {
    system_prompt: String,
    budget_tokens: usize,
    session_id: String,
    messages: Vec<ChatMessage>,
}

/// Opaque to the provider; only needs to be stable for one conversation.
fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos:x}", std::process::id())
}

impl Conversation {
    pub fn new(system_prompt: impl Into<String>, budget_tokens: usize) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            budget_tokens,
            session_id: new_session_id(),
            messages: Vec::new(),
        }
    }

    /// Restores a stored turn without going through the provider.
    pub fn push(&mut self, message: ChatMessage) {
        self.messages.push(message);
    }

    /// The assistant turn that asked for tools; the results need it in front,
    /// and the words she said before looking belong to that same turn.
    pub fn push_tool_round(&mut self, content: &str, calls: Vec<ToolCall>) {
        self.messages.push(ChatMessage::assistant_tools(content, calls));
    }

    /// Folds every observation that came back before the newest round into its
    /// first lines.
    ///
    /// One full observation costs about a quarter of the window, so three rounds
    /// of untouched results push the user's own question out of the prompt — and
    /// a model that cannot see the question keeps calling tools with no idea what
    /// it is answering.
    pub fn compact_observations(&mut self, keep: usize) {
        for message in &mut self.messages {
            if message.role != Role::Tool || message.content.chars().count() <= keep {
                continue;
            }
            let head: String = message.content.chars().take(keep).collect();
            message.content = format!("{head}… [recortado: leído en un turno anterior]");
        }
    }

    /// What one tool answered, bound to the call it belongs to.
    pub fn push_tool_result(&mut self, call_id: &str, content: impl Into<String>) {
        self.messages.push(ChatMessage::tool_result(call_id, content));
    }

    pub fn push_user(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage::user(content));
    }

    pub fn push_assistant(&mut self, content: impl Into<String>) {
        self.messages.push(ChatMessage::assistant(content));
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The prompt to send: trimmed history plus this conversation's session id.
    pub fn request(&self) -> ChatRequest {
        ChatRequest::new(self.trimmed(), self.session_id.clone())
    }

    /// The prompt with the running summary and the recalled episodes folded
    /// into the system message.
    ///
    /// They go there instead of as extra system turns because not every
    /// gateway treats multiple system messages the same way.
    pub fn request_with_context(&self, context: PromptContext<'_>) -> ChatRequest {
        let mut request = self.request();
        let mut extra = String::new();

        if let Some(memory) = context.memory.map(str::trim).filter(|text| !text.is_empty()) {
            extra.push_str("\n\nMemoria de la conversación hasta ahora (tu recuerdo, no del usuario):\n");
            extra.push_str(memory);
        }

        // The facts come before the fragments: what is still true matters more
        // than an episode from March.
        if let Some(cards) = context.cards.map(str::trim).filter(|text| !text.is_empty()) {
            extra.push_str(
                "\n\nLo que sabés sobre esto, guardado en tus tarjetas (son tuyos: no los repitas \
textualmente, usalos si vienen al caso, y si algo parece viejo decilo):\n",
            );
            extra.push_str(cards);
        }

        if let Some(recall) = context.recall.map(str::trim).filter(|text| !text.is_empty()) {
            extra.push_str(
                "\n\nFragmentos de conversaciones anteriores que pueden venir al caso. Son recuerdos \
tuyos: no los repitas textualmente, usalos solo si ayudan.\n",
            );
            extra.push_str(recall);
        }

        if !extra.is_empty()
            && let Some(system) = request.messages.first_mut()
        {
            system.content = format!("{}{extra}", self.system_prompt);
        }

        request
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Estimated tokens held by the stored turns, system prompt excluded.
    pub fn estimated_tokens(&self) -> usize {
        self.messages.iter().map(|message| estimate_tokens(&message.content)).sum()
    }

    /// System prompt plus the newest turns that fit the budget.
    ///
    /// Oldest turns are dropped first. The newest turn is always kept, even if it
    /// alone exceeds the budget: sending nothing is worse than sending too much.
    pub fn trimmed(&self) -> Vec<ChatMessage> {
        let mut used = estimate_tokens(&self.system_prompt);
        let mut kept: Vec<ChatMessage> = Vec::new();

        for message in self.messages.iter().rev() {
            let cost = estimate_tokens(&message.content);
            if !kept.is_empty() && used + cost > self.budget_tokens {
                break;
            }
            used += cost;
            kept.push(message.clone());
        }
        kept.reverse();

        // A tool result without the call it answers is a protocol error. The cut
        // happens at the oldest end, so only the head can be orphaned.
        while kept.first().is_some_and(|message| message.role == Role::Tool) {
            kept.remove(0);
        }

        let mut prompt = Vec::with_capacity(kept.len() + 1);
        prompt.push(ChatMessage::system(self.system_prompt.clone()));
        prompt.extend(kept);
        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::Role;

    #[test]
    fn keeps_everything_under_budget() {
        let mut conversation = Conversation::new("s", 10);
        conversation.push_user("hi");
        conversation.push_assistant("hello");

        let prompt = conversation.trimmed();
        assert_eq!(prompt.len(), 3);
        assert_eq!(prompt[0].role, Role::System);
        assert_eq!(prompt[1].content, "hi");
        assert_eq!(prompt[2].content, "hello");
    }

    #[test]
    fn drops_oldest_turns_first() {
        let mut conversation = Conversation::new("s", 5);
        for text in ["aaaa", "bbbb", "cccc", "dddd", "eeee"] {
            conversation.push_user(text);
        }

        let prompt = conversation.trimmed();
        assert_eq!(prompt.len(), 5, "system plus four newest turns");
        assert_eq!(prompt[1].content, "bbbb");
        assert_eq!(prompt[4].content, "eeee");
    }

    #[test]
    fn always_keeps_the_newest_turn() {
        let mut conversation = Conversation::new("s", 2);
        conversation.push_user("a".repeat(400));
        conversation.push_user("latest");

        let prompt = conversation.trimmed();
        assert_eq!(prompt.len(), 2);
        assert_eq!(prompt[1].content, "latest");
    }

    #[test]
    fn empty_conversation_is_just_the_system_prompt() {
        let conversation = Conversation::new("be nice", 100);
        let prompt = conversation.trimmed();
        assert_eq!(prompt.len(), 1);
        assert_eq!(prompt[0].role, Role::System);
    }

    /// Her words and her calls travel in the same assistant turn. Dropped words
    /// make the next round read as if she went silent and started calling tools
    /// out of nowhere, and she answers with an apology instead of the answer.
    #[test]
    fn a_tool_round_keeps_the_words_she_said() {
        let mut conversation = Conversation::new("s", 100);
        conversation.push_tool_round(
            "Voy a buscarlo.",
            vec![ToolCall {
                id: "call_1".to_owned(),
                name: "find".to_owned(),
                arguments: "{}".to_owned(),
            }],
        );
        conversation.push_tool_result("call_1", "encontrado");

        let prompt = conversation.trimmed();
        assert_eq!(prompt[1].role, Role::Assistant);
        assert_eq!(prompt[1].content, "Voy a buscarlo.");
        assert_eq!(prompt[1].tool_calls.as_ref().map(Vec::len), Some(1));
        assert_eq!(prompt[2].role, Role::Tool);
    }

    #[test]
    fn older_observations_are_folded_before_a_new_round() {
        let mut conversation = Conversation::new("s", 200);
        conversation.push_user("¿dónde está la novela?");
        conversation.push_tool_round(
            "Busco.",
            vec![ToolCall { id: "c1".to_owned(), name: "search".to_owned(), arguments: "{}".to_owned() }],
        );
        conversation.push_tool_result("c1", "x".repeat(3_000));

        conversation.compact_observations(400);

        let prompt = conversation.trimmed();
        let tool = prompt.iter().find(|message| message.role == Role::Tool).expect("tool result");
        assert!(tool.content.chars().count() < 500, "{}", tool.content.chars().count());
        assert!(tool.content.contains("recortado"));
        // The question is still there: that is what she was answering.
        assert!(prompt.iter().any(|message| message.content.contains("novela")));
    }

    #[test]
    fn each_context_block_lands_in_its_own_place() {
        let mut conversation = Conversation::new("system", 100);
        conversation.push_user("hola");

        let plain = conversation.request_with_context(PromptContext {
            memory: Some("el resumen"),
            ..PromptContext::default()
        });
        let system = &plain.messages[0].content;
        assert!(system.contains("el resumen"), "{system}");
        assert!(!system.contains("tarjetas"), "{system}");

        let full = conversation.request_with_context(PromptContext {
            memory: Some("el resumen"),
            cards: Some("### El faro (lore_novela)\n- Nerea vive ahí"),
            recall: Some("un fragmento viejo"),
        });
        let system = &full.messages[0].content;

        let memory_at = system.find("el resumen").expect("memory");
        let cards_at = system.find("lore_novela").expect("cards");
        let recall_at = system.find("un fragmento viejo").expect("recall");
        assert!(memory_at < cards_at && cards_at < recall_at, "{system}");
    }

    #[test]
    fn token_estimate_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
