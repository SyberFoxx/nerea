//! The one way in from outside the app: a named pipe.
//!
//! Signals were the obvious choice and the wrong one. This process embeds
//! WebKit, which keeps its own signal handlers, and `signal-hook-registry`
//! chains to the previous handler inside an async-signal-safe context — the
//! chain ended in JavaScriptCore and took the whole app down with a SIGSEGV,
//! three times, with the backtrace pointing at `libjavascriptcoregtk`.
//!
//! A FIFO needs no handler and nothing unsafe. `echo start > ptt` starts the
//! recording; `echo stop > ptt` ends it and asks for an answer. That is the
//! entire protocol, and any compositor, script or keybinding can speak it.

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
};

use tauri::Manager;

/// Where the pipe lives: next to the other runtime files, gone on logout.
fn pipe_path() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("nerea")
        .join("ptt")
}

/// Creates the pipe and listens on it forever, in its own thread.
///
/// A read on a FIFO blocks until somebody writes, which is exactly the shape of
/// this feature: nothing to do, then one line.
pub fn start(app: tauri::AppHandle) {
    let path = pipe_path();

    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        eprintln!("nerea ear: no puedo crear {}: {err}", parent.display());
        return;
    }

    // A stale pipe from a crashed run is not a pipe anybody can write to.
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    if let Err(err) = std::process::Command::new("mkfifo").arg(&path).status() {
        eprintln!("nerea ear: no puedo crear el tubo: {err}");
        return;
    }
    if !path.exists() {
        eprintln!("nerea ear: mkfifo no creó {}", path.display());
        return;
    }

    eprintln!("nerea ear: teclado armado en {}", path.display());
    std::thread::spawn(move || {
        loop {
            let Ok(file) = std::fs::File::open(&path) else {
                std::thread::sleep(std::time::Duration::from_millis(250));
                continue;
            };

            for line in BufReader::new(file).lines().map_while(Result::ok) {
                dispatch(&app, line.trim());
            }
        }
    });
}

/// One line, one action. Unknown words are ignored on purpose: the pipe is a
/// keybinding surface, and a typo there should not become an error dialogue.
fn dispatch(app: &tauri::AppHandle, command: &str) {
    match command {
        "start" => start_listening(app),
        "stop" => finish_listening(app),
        // One key, pressed twice: the shape most compositor binds can actually
        // express, since a release event is not always available to them.
        "toggle" => {
            if crate::hearing::is_recording() {
                finish_listening(app);
            } else {
                start_listening(app);
            }
        }
        _ => {}
    }
}

fn start_listening(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = crate::commands::listen_start(handle).await {
            eprintln!("nerea ear: {err}");
        }
    });
}

fn finish_listening(app: &tauri::AppHandle) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<crate::state::AppState>();
        let bank = handle.state::<nerea_core::PhraseBank>();
        match crate::commands::listen_turn(handle.clone(), state, bank).await {
            Ok(text) => eprintln!("nerea ear: escuché «{text}»"),
            Err(err) => eprintln!("nerea ear: {err}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipe_lives_under_the_runtime_dir() {
        let path = pipe_path();
        assert!(path.ends_with("nerea/ptt"), "{}", path.display());
    }
}
