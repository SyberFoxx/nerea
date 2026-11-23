use futures::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::state::{AppState, Status};

/// Event name the frontend listens on for streamed tokens.
const DELTA_EVENT: &str = "nerea://delta";

#[derive(Debug, Clone, Serialize)]
struct Delta {
    text: String,
}

#[derive(Debug, Serialize)]
pub struct Reply {
    text: String,
    provider: String,
    cancelled: bool,
    turns: usize,
}

#[tauri::command]
pub async fn status(state: State<'_, AppState>) -> Result<Status, String> {
    Ok(state.status().await)
}

#[tauri::command]
pub async fn reset_conversation(state: State<'_, AppState>) -> Result<Status, String> {
    state.reset().await;
    Ok(state.status().await)
}

#[tauri::command]
pub fn cancel_stream(state: State<'_, AppState>) {
    state.cancel();
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
) -> Result<Reply, String> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err("empty message".to_owned());
    }

    let provider = state.ensure_provider().await?;
    let request = state.begin_turn(&text).await;

    let mut stream = provider
        .stream_chat_completion(&request)
        .await
        .map_err(|err| err.to_string())?;

    let mut reply = String::new();
    let mut cancelled = false;

    while let Some(item) = stream.next().await {
        if state.take_cancel() {
            cancelled = true;
            break;
        }

        match item {
            Ok(delta) if delta.is_empty() => continue,
            Ok(delta) => {
                reply.push_str(&delta);
                // A closed window is not an error worth aborting the turn for.
                let _ = app.emit(DELTA_EVENT, Delta { text: delta });
            }
            Err(err) => {
                let turns = state.finish_turn(&reply).await;
                return Err(format!("{err} (after {turns} turns)"));
            }
        }
    }

    let turns = state.finish_turn(&reply).await;

    Ok(Reply {
        text: reply,
        provider: provider.id().to_owned(),
        cancelled,
        turns,
    })
}
