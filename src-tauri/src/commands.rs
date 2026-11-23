use std::process::Command;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};

use crate::state::AppState;

/// Event the avatar window follows.
const STATE_EVENT: &str = "nerea://state";

/// Gap between the avatar's feet and the host window edge.
const EDGE_INSET: i32 = 4;
/// Horizontal overlap so she looks seated on the corner, not floating beside it.
const SEAT_OVERLAP: i32 = 18;

#[derive(Debug, Clone, Serialize)]
struct AvatarState {
    state: &'static str,
}

/// Geometry of the window the avatar should sit on.
#[derive(Debug, Clone, Serialize)]
pub struct DockTarget {
    pub class: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

fn emit_state(app: &AppHandle, state: &'static str) {
    let _ = app.emit(STATE_EVENT, AvatarState { state });
}

fn hypr_json(args: &[&str]) -> Option<Value> {
    let output = Command::new("hyprctl").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn number(value: &Value, index: usize) -> Option<i32> {
    value.get(index)?.as_i64().map(|n| n as i32)
}

/// The focused client, ignoring our own windows. `None` outside Hyprland.
fn focused_client() -> Option<DockTarget> {
    let clients = hypr_json(&["clients", "-j"])?;
    let clients = clients.as_array()?;

    let mut best: Option<(i64, &Value)> = None;
    for client in clients {
        let class = client.get("class")?.as_str().unwrap_or_default();
        if class.to_ascii_lowercase().contains("nerea") {
            continue;
        }
        let is_floating_self = client.get("floating").and_then(Value::as_bool) == Some(true) && class.is_empty();
        if is_floating_self {
            continue;
        }
        let order = client.get("focusHistoryID").and_then(Value::as_i64).unwrap_or(i64::MAX);
        if best.is_none_or(|(current, _)| order < current) {
            best = Some((order, client));
        }
    }

    let (_, client) = best?;
    let at = client.get("at")?;
    let size = client.get("size")?;

    Some(DockTarget {
        class: client.get("class")?.as_str().unwrap_or_default().to_owned(),
        title: client.get("title")?.as_str().unwrap_or_default().to_owned(),
        x: number(at, 0)?,
        y: number(at, 1)?,
        width: number(size, 0)?,
        height: number(size, 1)?,
    })
}

fn monitor_bounds() -> Option<(i32, i32, i32, i32)> {
    let monitors = hypr_json(&["monitors", "-j"])?;
    let first = monitors.as_array()?.first()?;

    let work = first.get("x").and_then(Value::as_i64).zip(
        first.get("y").and_then(Value::as_i64),
    )?;
    let size = (
        first.get("width").and_then(Value::as_i64)?,
        first.get("height").and_then(Value::as_i64)?,
    );

    Some((work.0 as i32, work.1 as i32, size.0 as i32, size.1 as i32))
}

/// Parks the avatar on the bottom-right corner of the focused window.
///
/// Returns `None` on compositors without `hyprctl`; the overlay simply stays put.
#[tauri::command]
pub async fn dock(app: AppHandle) -> Result<Option<DockTarget>, String> {
    let target = focused_client();
    let avatar = app
        .get_webview_window("avatar")
        .ok_or_else(|| "avatar window is missing".to_owned())?;

    let Some(target) = target else {
        return Ok(None);
    };

    let size = avatar.outer_size().map_err(|err| err.to_string())?;
    let mut x = target.x + target.width - size.width as i32 + SEAT_OVERLAP;
    let mut y = target.y + target.height - size.height as i32 + EDGE_INSET;

    if let Some((mon_x, mon_y, mon_w, mon_h)) = monitor_bounds() {
        x = x.clamp(mon_x, mon_x + mon_w - size.width as i32);
        y = y.clamp(mon_y, mon_y + mon_h - size.height as i32);
    }

    avatar
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|err| err.to_string())?;

    Ok(Some(target))
}

#[derive(Debug, Serialize)]
pub struct Reply {
    text: String,
    provider: String,
    cancelled: bool,
    turns: usize,
}

#[derive(Debug, Clone, Serialize)]
struct Delta {
    text: String,
}

const DELTA_EVENT: &str = "nerea://delta";

#[tauri::command]
pub async fn status(state: tauri::State<'_, AppState>) -> Result<crate::state::Status, String> {
    Ok(state.status().await)
}

#[tauri::command]
pub async fn reset_conversation(
    state: tauri::State<'_, AppState>,
) -> Result<crate::state::Status, String> {
    state.reset().await;
    Ok(state.status().await)
}

#[tauri::command]
pub fn cancel_stream(state: tauri::State<'_, AppState>) {
    state.cancel();
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<Reply, String> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err("empty message".to_owned());
    }

    let provider = state.ensure_provider().await?;
    let request = state.begin_turn(&text).await;

    let mut stream = match provider.stream_chat_completion(&request).await {
        Ok(stream) => stream,
        Err(err) => {
            emit_state(&app, "error");
            return Err(err.to_string());
        }
    };

    emit_state(&app, "thinking");

    let mut reply = String::new();
    let mut cancelled = false;
    let mut talking = false;

    use futures::StreamExt;

    while let Some(item) = stream.next().await {
        if state.take_cancel() {
            cancelled = true;
            break;
        }

        match item {
            Ok(delta) if delta.is_empty() => continue,
            Ok(delta) => {
                if !talking {
                    talking = true;
                    emit_state(&app, "talking");
                }
                reply.push_str(&delta);
                let _ = app.emit(DELTA_EVENT, Delta { text: delta });
            }
            Err(err) => {
                state.finish_turn(&reply).await;
                emit_state(&app, "error");
                return Err(err.to_string());
            }
        }
    }

    let turns = state.finish_turn(&reply).await;
    emit_state(&app, "idle");

    Ok(Reply {
        text: reply,
        provider: provider.id().to_owned(),
        cancelled,
        turns,
    })
}
