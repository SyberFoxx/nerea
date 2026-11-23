//! Long-term memory: the conversation, condensed.
//!
//! The prompt only fits so much raw history, so the newest turns are sent as
//! they are and everything older is folded into one running summary. The
//! summary is what keeps her from forgetting the beginning of the relationship
//! as the transcript grows.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// How many messages stay raw in the prompt while the rest is summarized.
pub const KEEP_RAW: usize = 12;

/// How many unsummarized messages trigger a new summary.
pub const TRIGGER: usize = 24;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    /// The condensed conversation, in the user's language.
    #[serde(default)]
    pub text: String,
    /// Unix ms of the newest turn already folded into `text`.
    #[serde(default)]
    pub through: u64,
}

impl Memory {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn words(&self) -> usize {
        self.text.split_whitespace().count()
    }
}

/// How many of the pending turns to fold in, or `None` when it is not time yet.
pub fn plan(pending: usize) -> Option<usize> {
    (pending > TRIGGER).then(|| pending - KEEP_RAW)
}

pub struct MemoryStore {
    path: PathBuf,
}

impl MemoryStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `~/.local/share/nerea/memory.json`, or `NEREA_DATA_DIR`.
    pub fn open() -> Self {
        let dir = std::env::var("NEREA_DATA_DIR")
            .map(PathBuf::from)
            .ok()
            .or_else(|| ProjectDirs::from("", "", "nerea").map(|dirs| dirs.data_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(dir.join("memory.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A missing or damaged file reads as no memory at all: it is a cache of
    /// the transcript, never the only copy of anything.
    pub fn load(&self) -> Memory {
        fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, memory: &Memory) -> Result<(), std::io::Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let body = serde_json::to_vec_pretty(memory)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;

        // Write next to the target and rename, so a crash cannot leave half a
        // summary behind.
        let staging = self.path.with_extension("json.tmp");
        let mut file = fs::File::create(&staging)?;
        file.write_all(&body)?;
        file.sync_all()?;
        fs::rename(&staging, &self.path)
    }

    pub fn clear(&self) -> Result<(), std::io::Error> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> (PathBuf, MemoryStore) {
        let dir = std::env::temp_dir().join(format!("nerea-memory-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        (dir.clone(), MemoryStore::new(dir.join("memory.json")))
    }

    #[test]
    fn a_missing_file_means_no_memory() {
        let (dir, store) = scratch("missing");
        let memory = store.load();
        assert!(memory.is_empty());
        assert_eq!(memory.through, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn memory_round_trips() {
        let (dir, store) = scratch("round");
        let memory = Memory { text: "Lioner trabaja en Nerea.".to_owned(), through: 42 };
        store.save(&memory).expect("save");

        let loaded = store.load();
        assert_eq!(loaded.text, memory.text);
        assert_eq!(loaded.through, 42);
        assert_eq!(loaded.words(), 4);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_file_reads_as_no_memory() {
        let (dir, store) = scratch("damaged");
        fs::write(store.path(), "{ not json").expect("write");
        assert!(store.load().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn clearing_is_idempotent() {
        let (dir, store) = scratch("clear");
        store.save(&Memory { text: "algo".to_owned(), through: 1 }).expect("save");
        store.clear().expect("clear");
        assert!(store.load().is_empty());
        store.clear().expect("clear twice");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn only_a_full_backlog_triggers_a_summary() {
        assert_eq!(plan(0), None);
        assert_eq!(plan(TRIGGER), None);
        assert_eq!(plan(TRIGGER + 1), Some(TRIGGER + 1 - KEEP_RAW));
        assert_eq!(plan(40), Some(40 - KEEP_RAW));
    }
}
