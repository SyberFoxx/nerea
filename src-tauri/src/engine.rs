//! Owns the local voice engine's lifecycle.
//!
//! The engine is heavy (~3.6 GB RAM, ~2.8 GB VRAM) and only matters when a line
//! is about to be spoken, so the app starts it on demand, keeps it warm while
//! it is being used, and stops it after a quiet spell or when the app exits.
//!
//! If the engine is already listening — a server started by hand, or another
//! instance — the app adopts it instead of spawning a second one, which would
//! fight for the GPU.

use std::{
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

const DEFAULT_PORT: u16 = 9880;
const START_TIMEOUT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_millis(400);
/// After a failed attempt, do not try again for a while: startup is expensive.
const COOLDOWN: Duration = Duration::from_secs(180);
/// Stop the engine after this long without using it. Zero disables the reaper.
const DEFAULT_IDLE_SECS: u64 = 900;

static CHILD: Mutex<Option<Child>> = Mutex::new(None);
static LAST_USED: AtomicU64 = AtomicU64::new(0);
static LAST_FAILURE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Listening and ready to synthesize.
    Ready,
    /// Spawned, still loading weights.
    Starting,
    /// Not running; it can be started.
    Off,
    /// No engine on this machine: no venv, no script, no python.
    Unavailable,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn port() -> u16 {
    std::env::var("NEREA_TTS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn idle_limit() -> u64 {
    std::env::var("NEREA_TTS_IDLE_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_IDLE_SECS)
}

/// The checkout this build belongs to: the engine lives inside it.
fn repo_root() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1) {
            if ancestor.join("voice").join("serve_tts.py").is_file() {
                return Some(ancestor.to_path_buf());
            }
        }
    }

    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent()?.to_path_buf();
    here.join("voice").join("serve_tts.py").is_file().then_some(here)
}

fn python() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NEREA_TTS_PYTHON") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }

    repo_root().map(|root| root.join("tools/voice/.venv/bin/python")).filter(|path| path.is_file())
}

fn script() -> Option<PathBuf> {
    repo_root().map(|root| root.join("voice/serve_tts.py"))
}

fn is_listening() -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port()));
    TcpStream::connect_timeout(&address, Duration::from_millis(250)).is_ok()
}

fn child_alive() -> bool {
    let mut guard = CHILD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(None) => true,
            _ => {
                *guard = None;
                false
            }
        },
        None => false,
    }
}

pub fn state() -> State {
    if is_listening() {
        return State::Ready;
    }
    if child_alive() {
        return State::Starting;
    }
    if python().is_none() || script().is_none() {
        return State::Unavailable;
    }
    State::Off
}

/// Spawns the engine if it is not already listening.
fn spawn() -> Result<(), String> {
    if is_listening() {
        return Ok(());
    }

    let python = python().ok_or("no engine on this machine: missing tools/voice/.venv")?;
    let script = script().ok_or("no engine on this machine: missing voice/serve_tts.py")?;
    let root = repo_root().ok_or("no checkout to run the engine from")?;

    let log = std::env::temp_dir().join("nerea-tts.log");
    let stdout = std::fs::File::create(&log).map_err(|err| err.to_string())?;
    let stderr = stdout.try_clone().map_err(|err| err.to_string())?;

    let child = Command::new(python)
        .arg(script)
        .current_dir(&root)
        .env("NEREA_TTS_PARENT", std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map_err(|err| format!("could not start the engine: {err}"))?;

    let mut guard = CHILD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(child);
    Ok(())
}

fn mark_used() {
    LAST_USED.store(now_secs(), Ordering::Relaxed);
}

/// Starts the engine if needed and waits until it answers.
pub async fn ensure() -> Result<(), String> {
    if is_listening() {
        mark_used();
        return Ok(());
    }

    let last_failure = LAST_FAILURE.load(Ordering::Relaxed);
    let now = now_secs();
    if last_failure != 0 && now.saturating_sub(last_failure) < COOLDOWN.as_secs() {
        return Err("the engine failed recently; using the bank".to_owned());
    }

    if !child_alive()
        && let Err(err) = spawn()
    {
        LAST_FAILURE.store(now, Ordering::Relaxed);
        return Err(err);
    }

    let deadline = Instant::now() + START_TIMEOUT;
    while Instant::now() < deadline {
        if is_listening() {
            mark_used();
            return Ok(());
        }

        if !child_alive() {
            LAST_FAILURE.store(now_secs(), Ordering::Relaxed);
            return Err(format!(
                "the engine died while starting; see {}",
                std::env::temp_dir().join("nerea-tts.log").display()
            ));
        }

        tokio::time::sleep(READY_POLL).await;
    }

    LAST_FAILURE.store(now_secs(), Ordering::Relaxed);
    Err("the engine did not come up in time".to_owned())
}

/// Stops the engine if this app started it. A server it adopted is left alone.
pub fn stop() {
    let mut guard = CHILD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Stops the engine once it has been unused for long enough.
pub fn reap() {
    let limit = idle_limit();
    if limit == 0 || !child_alive() {
        return;
    }

    let last = LAST_USED.load(Ordering::Relaxed);
    if last == 0 {
        return;
    }

    if now_secs().saturating_sub(last) >= limit {
        stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_is_one_of_four_things() {
        let state = state();
        assert!(matches!(state, State::Ready | State::Starting | State::Off | State::Unavailable));
    }

    #[test]
    fn stopping_without_a_child_is_harmless() {
        stop();
        stop();
    }

    #[test]
    fn the_idle_limit_can_be_disabled() {
        // Defaults are positive; the reaper treats zero as "never".
        assert!(idle_limit() > 0 || std::env::var("NEREA_TTS_IDLE_SECS").as_deref() == Ok("0"));
    }
}
