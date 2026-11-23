use std::{path::PathBuf, process::Command};

use nerea_core::PhraseBank;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, LogicalSize, Manager};

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

/// Our own avatar window as the compositor sees it.
///
/// GTK does not report the frame it wraps around the webview, and Wayland has
/// no client-side positioning, so the compositor is the only source of truth
/// for both geometry and placement.
#[derive(Debug, Clone)]
struct OwnWindow {
    address: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn own_window() -> Option<OwnWindow> {
    let clients = hypr_json(&["clients", "-j"])?;
    let pid = std::process::id() as i64;

    clients.as_array()?.iter().find_map(|client| {
        if client.get("pid").and_then(Value::as_i64)? != pid {
            return None;
        }
        if client.get("title").and_then(Value::as_str)? != "nerea-avatar" {
            return None;
        }
        let at = client.get("at")?;
        let size = client.get("size")?;

        Some(OwnWindow {
            address: client.get("address")?.as_str()?.to_owned(),
            x: number(at, 0)?,
            y: number(at, 1)?,
            width: number(size, 0)?,
            height: number(size, 1)?,
        })
    })
}

/// Wayland forbids a client from placing its own window; the compositor can.
fn place_window(frame: &OwnWindow, x: i32, y: i32) {
    let expression = format!(
        "hl.dsp.window.move({{ x = {x}, y = {y}, window = \"address:{}\" }})",
        frame.address
    );
    let _ = Command::new("hyprctl").arg("dispatch").arg(expression).output();
}

/// Pins the window to an exact size.
///
/// GTK ignores a resize on the non-resizable window the compositor needs in
/// order to keep her floating, so the size goes in as an identical minimum and
/// maximum instead.
fn pin_size(window: &tauri::WebviewWindow, size: (f64, f64)) -> Result<(), String> {
    let size = tauri::Size::Logical(LogicalSize::new(size.0, size.1));
    window.set_min_size(Some(size)).map_err(|err| err.to_string())?;
    window.set_max_size(Some(size)).map_err(|err| err.to_string())
}

/// Top-left corner that keeps her bottom-right corner where it already is.
fn anchored(frame: &OwnWindow, size: (f64, f64), scale: f64) -> (i32, i32, i32, i32) {
    let width = (size.0 * scale).round() as i32;
    let height = (size.1 * scale).round() as i32;
    let mut x = frame.x + frame.width - width;
    let mut y = frame.y + frame.height - height;

    if let Some((mon_x, mon_y, mon_w, mon_h)) = monitor_bounds() {
        x = x.clamp(mon_x, mon_x + mon_w - width);
        y = y.clamp(mon_y, mon_y + mon_h - height);
    }

    (x, y, width, height)
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
    let avatar = app
        .get_webview_window("avatar")
        .ok_or_else(|| "avatar window is missing".to_owned())?;
    let scale = avatar.scale_factor().unwrap_or(1.0);

    pin_size(&avatar, PORTRAIT)?;

    let Some(frame) = own_window() else {
        return Ok(None);
    };

    let width = (PORTRAIT.0 * scale).round() as i32;
    let height = (PORTRAIT.1 * scale).round() as i32;
    let target = focused_client();

    let (mut x, mut y) = match target.as_ref() {
        Some(target) => (
            target.x + target.width - width + SEAT_OVERLAP,
            target.y + target.height - height + EDGE_INSET,
        ),
        None => (frame.x, frame.y),
    };

    if let Some((mon_x, mon_y, mon_w, mon_h)) = monitor_bounds() {
        x = x.clamp(mon_x, mon_x + mon_w - width);
        y = y.clamp(mon_y, mon_y + mon_h - height);
    }

    place_window(&frame, x, y);

    Ok(target)
}

/// Her portrait alone, and the same window with the chat panel open.
const PORTRAIT: (f64, f64) = (176.0, 256.0);
const PANEL: (f64, f64) = (560.0, 560.0);

/// Grows or shrinks the avatar window, keeping her feet where they are.
///
/// Anchoring bottom-right means the panel opens up and to the left, so her
/// position on screen does not change when the chat appears.
fn resize_panel(app: &AppHandle, open: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("avatar")
        .ok_or_else(|| "avatar window is missing".to_owned())?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let target = if open { PANEL } else { PORTRAIT };

    if let Some(frame) = own_window() {
        let (x, y, _, _) = anchored(&frame, target, scale);
        pin_size(&window, target)?;
        place_window(&frame, x, y);
    } else {
        pin_size(&window, target)?;
    }

    if open {
        let _ = window.set_focus();
    }

    Ok(())
}

/// Opens the chat panel: the window grows, she does not move.
#[tauri::command]
pub async fn open_panel(app: AppHandle) -> Result<(), String> {
    resize_panel(&app, true)
}

/// Back to her portrait, so the window stops covering the desktop.
#[tauri::command]
pub async fn close_panel(app: AppHandle) -> Result<(), String> {
    resize_panel(&app, false)
}

/// The read-only history window is never opened automatically.
#[tauri::command]
pub async fn open_history(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("history")
        .ok_or_else(|| "history window is missing".to_owned())?;

    // She is always on top, and the history needs to be readable.
    if let Some(avatar) = app.get_webview_window("avatar") {
        let _ = avatar.hide();
    }

    window.show().map_err(|err| err.to_string())?;
    window.set_focus().map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn close_history(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("history")
        .ok_or_else(|| "history window is missing".to_owned())?;
    let hidden = window.hide().map_err(|err| err.to_string());

    if let Some(avatar) = app.get_webview_window("avatar") {
        let _ = avatar.show();
    }

    hidden
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub role: nerea_core::Role,
    pub content: String,
    /// Unix milliseconds.
    pub at: u64,
    pub tokens: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
    pub messages: usize,
    pub tokens: usize,
    /// The running summary: everything the prompt no longer carries raw.
    pub memory: String,
    pub status: crate::state::Status,
}

fn stored_entries(state: &AppState) -> Vec<HistoryEntry> {
    state
        .transcript_entries()
        .into_iter()
        .map(|entry| HistoryEntry {
            tokens: nerea_core::estimate_tokens(&entry.content),
            role: entry.role,
            content: entry.content,
            at: entry.at,
        })
        .collect()
}

/// Everything ever stored plus the running totals the history window shows.
#[tauri::command]
pub async fn history(state: tauri::State<'_, AppState>) -> Result<History, String> {
    let entries = stored_entries(&state);

    Ok(History {
        messages: entries.len(),
        tokens: entries.iter().map(|entry| entry.tokens).sum(),
        entries,
        memory: state.memory_text().await,
        status: state.status().await,
    })
}

/// The newest `limit` turns, for the short log inside the panel.
#[tauri::command]
pub async fn transcript(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<HistoryEntry>, String> {
    let mut entries = stored_entries(&state);

    if let Some(limit) = limit {
        let keep_from = entries.len().saturating_sub(limit);
        entries.drain(..keep_from);
    }

    Ok(entries)
}

/// First line after launch: a resume line when there is history, a boot line
/// when this is the first time.
#[tauri::command]
pub fn greet(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    bank: tauri::State<'_, PhraseBank>,
) -> Option<Said> {
    let category = if state.has_memory() { "resume" } else { "boot" };
    speak(&app, &bank, category)
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
const UPDATE_EVENT: &str = "nerea://update";
const MEMORY_EVENT: &str = "nerea://memory";
const RECALL_EVENT: &str = "nerea://recall";
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

/// Where the rendered lines live.
///
/// A packaged build cannot read the checkout, so it looks next to the bundle's
/// resources before falling back to the working directory.
fn bank_dir(app: &AppHandle) -> PathBuf {
    if let Ok(dir) = std::env::var("NEREA_BANK_DIR") {
        return PathBuf::from(dir);
    }

    if let Ok(resources) = app.path().resource_dir() {
        let candidate = resources.join("voice").join("bank");
        if candidate.is_dir() {
            return candidate;
        }
    }

    PathBuf::from("assets/voice/bank")
}

pub(crate) fn speak(app: &AppHandle, bank: &PhraseBank, category: &str) -> Option<Said> {
    let phrase = bank.pick(category)?;
    let path = bank_dir(app).join(format!("{}.wav", phrase.id));
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
        duration_ms: crate::voice::wav_duration_ms(&path).unwrap_or(1_800),
        played,
    };

    let _ = app.emit(SAY_EVENT, &said);
    Some(said)
}

/// What the panel needs to offer the update entry, or to hide it.
#[tauri::command]
pub async fn update_status() -> Result<crate::update::UpdateStatus, String> {
    tauri::async_runtime::spawn_blocking(crate::update::status)
        .await
        .map_err(|err| err.to_string())
}

/// Pulls the checkout and rebuilds. The caller restarts into the new binary.
#[tauri::command]
pub async fn update_apply(app: AppHandle) -> Result<(), String> {
    let _ = app.emit(UPDATE_EVENT, "pull");

    if let Err(err) = tauri::async_runtime::spawn_blocking(crate::update::pull)
        .await
        .map_err(|err| err.to_string())?
    {
        let _ = app.emit(UPDATE_EVENT, err.clone());
        return Err(err);
    }

    let _ = app.emit(UPDATE_EVENT, "build");

    if let Err(err) = tauri::async_runtime::spawn_blocking(crate::update::build)
        .await
        .map_err(|err| err.to_string())?
    {
        let _ = app.emit(UPDATE_EVENT, err.clone());
        return Err(err);
    }

    let _ = app.emit(UPDATE_EVENT, "restarting");
    app.restart()
}

/// Voice engine state, for the menu.
#[tauri::command]
pub fn voice_state() -> crate::engine::State {
    crate::engine::state()
}

/// `None` means the question was never answered, and she stays silent.
#[tauri::command]
pub fn voice_preference() -> Result<Option<bool>, String> {
    nerea_core::Config::load()
        .map(|config| config.voice_answer())
        .map_err(|err| err.to_string())
}

/// Records the answer. Turning speech off stops the engine immediately.
#[tauri::command]
pub async fn voice_set_preference(on: bool) -> Result<(), String> {
    let mut config = nerea_core::Config::load().map_err(|err| err.to_string())?;
    config.set_speech(on);
    config.save().map_err(|err| err.to_string())?;

    if !on {
        crate::engine::stop();
    }

    Ok(())
}

/// The reply, said in Japanese: one small call, faithful to the Spanish text.
pub(crate) async fn japanese_line(
    provider: &std::sync::Arc<dyn nerea_core::AiProvider>,
    reply: &str,
) -> Option<String> {
    const PROMPT: &str = "Traducí al japonés hablado, natural y breve, esta respuesta de Nerea. \
Máximo 60 caracteres. Reglas: llamá al usuario 私の竜 o 愛しの竜, nunca ドラゴン; conservá su tic \
あらあら si aparece; no agregues nada que no esté en la respuesta. Devolvé solo la línea: sin comillas, \
sin romaji, sin explicación.";

    let request = nerea_core::ChatRequest {
        messages: vec![
            nerea_core::ChatMessage::system(PROMPT),
            nerea_core::ChatMessage::user(reply.chars().take(1_000).collect::<String>()),
        ],
        session_id: "voice".to_owned(),
    };

    let answer = provider.chat_completion(&request).await.ok()?;
    let line = answer.content.trim().trim_matches('"').trim().to_owned();

    (!line.is_empty()).then_some(line)
}

/// One short extra call: the tone of the reply decides which reaction plays.
async fn mood(provider: &std::sync::Arc<dyn nerea_core::AiProvider>, reply: &str) -> Option<&'static str> {
    const PROMPT: &str = "Clasificá el tono de la respuesta de un asistente en UNA sola palabra. \
Opciones: neutral, bromista, carinosa, sorprendida. Respondé solo la palabra, sin explicar.";

    let request = nerea_core::ChatRequest {
        messages: vec![
            nerea_core::ChatMessage::system(PROMPT),
            nerea_core::ChatMessage::user(reply.chars().take(600).collect::<String>()),
        ],
        session_id: "mood".to_owned(),
    };

    let answer = provider.chat_completion(&request).await.ok()?;
    let word = answer.content.trim().to_lowercase();

    if word.contains("bromista") {
        Some("tease")
    } else if word.contains("carinosa") || word.contains("cariñosa") {
        Some("affection")
    } else if word.contains("sorprendida") {
        Some("surprise")
    } else {
        None
    }
}

/// Plays one goodbye line when the app is closing. Detached audio outlives us.
pub(crate) fn farewell(app: &AppHandle) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SAID.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if let Some(bank) = app.try_state::<PhraseBank>() {
        speak(app, &bank, "goodbye");
    }
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
    let turn = state.begin_turn(&text).await;
    let request = turn.request;

    // Speaking is opt-in: it costs gigabytes, so it only happens after the user
    // answered the question, and the engine boots while the model is thinking.
    let speech = nerea_core::Config::load().map(|config| config.speech_enabled()).unwrap_or(false);
    if speech {
        tauri::async_runtime::spawn(async {
            let _ = crate::engine::ensure().await;
        });
    }

    if turn.recalled > 0 && turn.oldest_days >= 1 {
        let _ = app.emit(RECALL_EVENT, turn.recalled);
    }

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

    // Her actual voice first; the bank is the fallback when the engine is off.
    let mut spoke_live = false;

    if speech && !cancelled && !reply.trim().is_empty()
        && let Some(reference) = bank.reference_prompt().map(str::to_owned)
        && crate::engine::ensure().await.is_ok()
        && let Some(japanese) = japanese_line(&provider, &reply).await
    {
        match crate::voice::speak(&japanese, &reply, &reference).await {
            Ok(line) => {
                let _ = app.emit(SAY_EVENT, &line);
                spoke_live = true;
            }
            Err(_) => spoke_live = false,
        }
    }

    if !spoke_live && !cancelled && reply.chars().count() > 12
        && let Some(category) = mood(&provider, &reply).await
    {
        speak(&app, &bank, category);
    }

    // Folded in the background: the reply already reached the UI, and the
    // summary is one extra small call every dozen turns.
    if !cancelled {
        let state = state.inner().clone();
        let summarizer = provider.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(text) = state.summarize_if_due(&summarizer).await {
                let _ = app.emit(MEMORY_EVENT, text);
            }
        });
    }

    Ok(Reply {
        text: reply,
        provider: provider.id().to_owned(),
        cancelled,
        turns,
    })
}
