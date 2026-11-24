//! The audit log: one line per thing she looked at.
//!
//! Append-only and best-effort. A failed write is bookkeeping, never a reason
//! to break a read.

use std::io::Write;

use serde::Serialize;
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
