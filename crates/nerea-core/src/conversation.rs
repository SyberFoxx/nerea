use std::time::{SystemTime, UNIX_EPOCH};

use crate::ai::{ChatMessage, ChatRequest};

/// Rough characters-per-token ratio. Tokenizers differ per model, so this is a
/// cheap lower bound with margin instead of a per-provider dependency.
const CHARS_PER_TOKEN: usize = 4;

/// Deliberately approximate: the budget is a safety net, not an accounting system.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
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
        ChatRequest {
            messages: self.trimmed(),
            session_id: self.session_id.clone(),
        }
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
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

    #[test]
    fn token_estimate_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
