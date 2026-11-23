use std::{
    path::{Path, PathBuf},
    process::Command,
};

use nerea_core::PhraseBank;
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

/// Opens the small composer to the left of the avatar, anchored to her feet.
#[tauri::command]
pub async fn open_composer(app: AppHandle) -> Result<(), String> {
    let avatar = app
        .get_webview_window("avatar")
        .ok_or_else(|| "avatar window is missing".to_owned())?;
    let composer = app
        .get_webview_window("composer")
        .ok_or_else(|| "composer window is missing".to_owned())?;

    let avatar_pos = avatar.outer_position().map_err(|err| err.to_string())?;
    let avatar_size = avatar.outer_size().map_err(|err| err.to_string())?;
    let composer_size = composer.outer_size().map_err(|err| err.to_string())?;

    // GTK reports a real size once the hidden window exists, but fall back to the
    // configured size (plus its known padding) if the first measurement is empty.
    let width = if composer_size.width < 100 { 408 } else { composer_size.width as i32 };
    let height = if composer_size.height < 100 { 348 } else { composer_size.height as i32 };

    // Overlap her body slightly so the bubble reads as coming from her.
    let mut x = avatar_pos.x - width + 96;
    let mut y = avatar_pos.y + avatar_size.height as i32 - height;

    if let Some((mon_x, mon_y, mon_w, mon_h)) = monitor_bounds() {
        x = x.clamp(mon_x, mon_x + mon_w - width);
        y = y.clamp(mon_y, mon_y + mon_h - height);
    }

    composer
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|err| err.to_string())?;
    composer.show().map_err(|err| err.to_string())?;
    composer.set_focus().map_err(|err| err.to_string())?;

    Ok(())
}

#[tauri::command]
pub async fn close_composer(app: AppHandle) -> Result<(), String> {
    let composer = app
        .get_webview_window("composer")
        .ok_or_else(|| "composer window is missing".to_owned())?;
    composer.hide().map_err(|err| err.to_string())
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
const SAY_EVENT: &str = "nerea://say";

/// One spoken line: the audio already exists, so playing it costs nothing.
#[derive(Debug, Clone, Serialize)]
pub struct Said {
    pub id: String,
    pub category: String,
    pub ja: String,
    pub es: String,
    pub duration_ms: u64,
    pub played: bool,
}

fn bank_dir() -> PathBuf {
    std::env::var("NEREA_BANK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("assets/voice/bank"))
}

/// Reads duration straight from the WAV headers instead of spawning ffprobe.
fn wav_duration_ms(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut offset = 12;
    let (mut channels, mut rate, mut bits) = (1u64, 32_000u64, 16u64);

    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let body = offset + 8;

        if id == b"fmt " && body + 16 <= bytes.len() {
            channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]) as u64;
            rate = u32::from_le_bytes([
                bytes[body + 4],
                bytes[body + 5],
                bytes[body + 6],
                bytes[body + 7],
            ]) as u64;
            bits = u16::from_le_bytes([bytes[body + 14], bytes[body + 15]]) as u64;
        } else if id == b"data" {
            let bytes_per_second = rate * channels * bits.max(8) / 8;
            if bytes_per_second == 0 {
                return None;
            }
            let available = size.min(bytes.len().saturating_sub(body)) as u64;
            return Some(available * 1000 / bytes_per_second);
        }

        offset = body + size + (size & 1);
    }

    None
}

fn speak(app: &AppHandle, bank: &PhraseBank, category: &str) -> Option<Said> {
    let phrase = bank.pick(category)?;
    let path = bank_dir().join(format!("{}.wav", phrase.id));
    let played = path.exists();

    if played {
        // Detached: the UI must not wait for the audio to finish.
        let _ = Command::new("pw-play").arg(&path).spawn();
    }

    let said = Said {
        id: phrase.id,
        category: phrase.category,
        ja: phrase.ja,
        es: phrase.es,
        duration_ms: wav_duration_ms(&path).unwrap_or(1_800),
        played,
    };

    let _ = app.emit(SAY_EVENT, &said);
    Some(said)
}

#[tauri::command]
pub fn say(
    app: AppHandle,
    bank: tauri::State<'_, PhraseBank>,
    category: String,
) -> Result<Option<Said>, String> {
    Ok(speak(&app, &bank, &category))
}

#[tauri::command]
pub fn say_categories(bank: tauri::State<'_, PhraseBank>) -> Vec<String> {
    bank.categories().into_iter().map(str::to_owned).collect()
}

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
    bank: tauri::State<'_, PhraseBank>,
    text: String,
) -> Result<Reply, String> {
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err("empty message".to_owned());
    }

    let provider = match state.ensure_provider().await {
        Ok(provider) => provider,
        Err(err) => {
            speak(&app, &bank, "complain");
            return Err(err);
        }
    };
    let request = state.begin_turn(&text).await;

    let mut stream = match provider.stream_chat_completion(&request).await {
        Ok(stream) => stream,
        Err(err) => {
            emit_state(&app, "error");
            speak(&app, &bank, "complain");
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
                speak(&app, &bank, "complain");
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
