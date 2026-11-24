//! The audit log: one line per thing she looked at.
//!
//! Append-only and best-effort. A failed write is bookkeeping, never a reason
//! to break a read.

use std::io::Write;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
struct Line<'a> {
    at: u64,
    tool: &'a str,
    args: &'a Value,
    ok: bool,
    bytes: usize,
    summary: &'a str,
}

pub fn append(tool: &str, args: &Value, summary: &str, ok: bool, bytes: usize) {
    // The unit tests call `run` for every tool; their calls are noise in a log
    // meant for the user's eyes.
    if cfg!(test) {
        return;
    }

    let dir = crate::paths::data_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }

    let Ok(mut line) = serde_json::to_string(&Line { at: now_ms(), tool, args, ok, bytes, summary })
    else {
        return;
    };
    line.push('\n');

    if let Ok(mut file) =
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join("audit.jsonl"))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// One line of the log, as the history window shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    pub at: u64,
    pub tool: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub bytes: usize,
    #[serde(default)]
    pub summary: String,
}

/// The last `limit` calls, oldest first. A line we cannot read is skipped: the
/// log is append-only and a half-written line is not a reason to show nothing.
pub fn recent(limit: usize) -> Vec<Call> {
    recent_at(&crate::paths::data_dir().join("audit.jsonl"), limit)
}

pub fn recent_at(path: &std::path::Path, limit: usize) -> Vec<Call> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    let mut calls: Vec<Call> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Call>(line).ok())
        .collect();

    if calls.len() > limit {
        calls.drain(..calls.len() - limit);
    }
    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nerea-audit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("audit.jsonl")
    }

    #[test]
    fn recent_keeps_the_newest_calls_and_skips_a_broken_line() {
        let path = scratch("recent");
        let body = concat!(
            r#"{"at":1,"tool":"find","ok":true,"bytes":10,"summary":"buscando el archivo «nerea»"}"#,
            "\n",
            "not json at all\n",
            r#"{"at":2,"tool":"search","ok":false,"bytes":0,"summary":"buscando «x»"}"#,
            "\n",
        );
        std::fs::write(&path, body).expect("write");

        let calls = recent_at(&path, 5);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool, "find");
        assert_eq!(calls[1].tool, "search");
        assert!(!calls[1].ok);

        let newest = recent_at(&path, 1);
        assert_eq!(newest.len(), 1);
        assert_eq!(newest[0].tool, "search");
    }

    #[test]
    fn a_missing_log_is_empty_not_an_error() {
        let path = scratch("missing").with_file_name("nope.jsonl");
        assert!(recent_at(&path, 10).is_empty());
    }
}
