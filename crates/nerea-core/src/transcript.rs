//! On-disk conversation history.
//!
//! Append-only JSON lines: one entry per turn, so a crash loses at most the
//! line being written. Reading is best effort: a damaged line is skipped
//! instead of discarding the whole file.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::ai::{ChatMessage, Role};

/// One stored line of the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub role: Role,
    pub content: String,
    /// Unix milliseconds, so the history view can show when it happened.
    #[serde(default)]
    pub at: u64,
}

impl Entry {
    pub fn message(&self) -> ChatMessage {
        ChatMessage { role: self.role, content: self.content.clone() }
    }
}

/// Where the conversation lives between runs.
pub struct Transcript {
    path: PathBuf,
}

impl Transcript {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Default store: `~/.local/share/nerea/conversation.jsonl`.
    pub fn open() -> Self {
        let path = ProjectDirs::from("", "", "nerea")
            .map(|dirs| dirs.data_dir().join("conversation.jsonl"))
            .unwrap_or_else(|| PathBuf::from("conversation.jsonl"));
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Appends one turn. Empty content is a no-op, not an error.
    pub fn append(&self, role: Role, content: &str) -> Result<(), std::io::Error> {
        let content = content.trim();
        if content.is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let entry = Entry { role, content: content.to_owned(), at: now_ms() };
        let mut line = serde_json::to_string(&entry)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        line.push('\n');

        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        file.write_all(line.as_bytes())
    }

    /// Every readable entry, oldest first. Missing file means empty history.
    pub fn entries(&self) -> Vec<Entry> {
        let Ok(bytes) = fs::read(&self.path) else {
            return Vec::new();
        };

        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| serde_json::from_str::<Entry>(line.trim()).ok())
            .filter(|entry| !entry.content.trim().is_empty())
            .collect()
    }

    /// The newest `limit` messages, oldest first, ready to restore a prompt.
    pub fn recent(&self, limit: usize) -> Vec<ChatMessage> {
        let all = self.entries();
        let start = all.len().saturating_sub(limit);
        all[start..]
            .iter()
            .filter(|entry| entry.role != Role::System)
            .map(Entry::message)
            .collect()
    }

    /// Erases the file. A missing file is already the desired state.
    pub fn clear(&self) -> Result<(), std::io::Error> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, Transcript) {
        let dir = std::env::temp_dir().join(format!(
            "nerea-transcript-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("conversation.jsonl");
        (dir, Transcript::new(path))
    }

    #[test]
    fn appending_then_reading_keeps_order() {
        let (dir, transcript) = scratch("order");
        transcript.append(Role::User, "hola").expect("user turn");
        transcript.append(Role::Assistant, "arara").expect("reply");

        let messages = transcript.recent(10);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[0].content, "hola");
        assert_eq!(messages[1].content, "arara");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_keeps_the_newest_messages() {
        let (dir, transcript) = scratch("recent");
        for text in ["uno", "dos", "tres", "cuatro"] {
            transcript.append(Role::User, text).expect("turn");
        }

        let messages = transcript.recent(2);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "tres");
        assert_eq!(messages[1].content, "cuatro");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_line_is_skipped_not_fatal() {
        let (dir, transcript) = scratch("damaged");
        transcript.append(Role::User, "buena").expect("turn");
        fs::write(
            transcript.path(),
            format!(
                "{}\n{{ not json\n{}\n",
                r#"{"role":"user","content":"buena","at":1}"#,
                r#"{"role":"assistant","content":"otra","at":2}"#
            ),
        )
        .expect("write");

        let entries = transcript.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].content, "otra");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_content_is_not_stored() {
        let (dir, transcript) = scratch("empty");
        transcript.append(Role::Assistant, "   ").expect("no-op");
        assert!(transcript.entries().is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn clearing_removes_history_and_tolerates_a_missing_file() {
        let (dir, transcript) = scratch("clear");
        transcript.append(Role::User, "hola").expect("turn");
        transcript.clear().expect("clear");
        assert!(transcript.entries().is_empty());
        transcript.clear().expect("clearing twice is fine");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_reads_as_empty() {
        let (dir, transcript) = scratch("missing");
        assert!(transcript.entries().is_empty());

        let _ = fs::remove_dir_all(dir);
    }
}
