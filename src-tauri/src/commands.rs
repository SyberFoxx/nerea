use std::{path::PathBuf, process::Command, sync::Arc};

use nerea_core::{AiProvider, ChatRequest, PhraseBank};
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
pub(crate) struct OwnWindow {
    pub(crate) address: String,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: i32,
    pub(crate) height: i32,
}

pub(crate) fn own_window(title: &str) -> Option<OwnWindow> {
    let clients = hypr_json(&["clients", "-j"])?;
    let pid = std::process::id() as i64;

    clients.as_array()?.iter().find_map(|client| {
        if client.get("pid").and_then(Value::as_i64)? != pid {
            return None;
        }
        if client.get("title").and_then(Value::as_str)? != title {
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

/// Her portrait and the chat are two windows; only one is on screen at a time.
fn active_title(app: &AppHandle) -> &'static str {
    match app.get_webview_window("chat") {
        Some(chat) if chat.is_visible().unwrap_or(false) => "nerea-chat",
        _ => "nerea-avatar",
    }
}

fn active_window(app: &AppHandle) -> Option<OwnWindow> {
    own_window(active_title(app))
}

/// Wayland forbids a client from placing its own window; the compositor can.
fn place_window(frame: &OwnWindow, x: i32, y: i32) {
    let expression = format!(
        "hl.dsp.window.move({{ x = {x}, y = {y}, window = \"address:{}\" }})",
        frame.address
    );
    let _ = Command::new("hyprctl").arg("dispatch").arg(expression).output();
}

/// Her portrait alone, and the same window with the chat open.
const PORTRAIT: (f64, f64) = (176.0, 256.0);
const PANEL: (f64, f64) = (600.0, 600.0);

/// Pins the window to an exact size.
///
/// A resizable window is the only one GTK will resize at all, and a fixed size
/// is what keeps her floating instead of being tiled, so the size goes in as an
/// identical minimum and maximum — the compositor treats that as fixed, and GTK
/// still recomputes when the bounds change.
async fn pin_size(window: &tauri::WebviewWindow, size: (f64, f64)) -> Result<(), String> {
    let target = tauri::Size::Logical(LogicalSize::new(size.0, size.1));

    window.set_resizable(true).map_err(|err| err.to_string())?;
    window.set_min_size(Some(target)).map_err(|err| err.to_string())?;
    window.set_max_size(Some(target)).map_err(|err| err.to_string())?;

    // Let GTK apply it before anything reads the geometry back.
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    Ok(())
}

/// Grows or shrinks the avatar window, keeping her feet where they are.
///
/// Her portrait sits in the bottom-right corner of the window, so holding that
/// corner still is what keeps her on screen: the chat opens up and to the left.
/// The placement is computed from the geometry *before* the resize, because the
/// compositor keeps a floating window's centre when its size changes — place
/// her where the resize will leave her, not where she is now.
async fn resize_panel(app: &AppHandle, open: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("avatar")
        .ok_or_else(|| "avatar window is missing".to_owned())?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let target = if open { PANEL } else { PORTRAIT };

    let frame = own_window("nerea-avatar");

    // The compositor resizes a floating window around its centre, and the move
    // that keeps her corner is a separate request: the window is placed where
    // the resize will leave her, so both land in the same breath. The page
    // hides itself for the swap, so none of it is ever drawn.
    if let Some(frame) = frame.as_ref() {
        let (x, y, width, height) = anchored(frame, target, scale);
        let pre_x = x + (width - frame.width) / 2;
        let pre_y = y + (height - frame.height) / 2;
        place_window(frame, pre_x, pre_y);
    }

    pin_size(&window, target).await?;

    if open {
        let _ = window.set_focus();
    }

    Ok(())
}

/// Opens the chat: the window grows around her, she does not move.
#[tauri::command]
pub async fn open_panel(app: AppHandle) -> Result<(), String> {
    resize_panel(&app, true).await
}

/// Back to her portrait, so the window stops covering the desktop.
#[tauri::command]
pub async fn close_panel(app: AppHandle) -> Result<(), String> {
    resize_panel(&app, false).await
}

/// Top-left corner that keeps her bottom-right corner where it already is.
fn anchored(frame: &OwnWindow, size: (f64, f64), scale: f64) -> (i32, i32, i32, i32) {
    let width = (size.0 * scale).round() as i32;
    let height = (size.1 * scale).round() as i32;
    let mut x = frame.x + frame.width - width;
    let mut y = frame.y + frame.height - height;

    let centre = (frame.x + frame.width / 2, frame.y + frame.height / 2);
    if let Some((mon_x, mon_y, mon_w, mon_h)) = monitor_at(centre.0, centre.1) {
        x = x.clamp(mon_x, mon_x + mon_w - width);
        y = y.clamp(mon_y, mon_y + mon_h - height);
    }

    (x, y, width, height)
}

/// The monitor holding a point: she has to stay where the person looking at her
/// is looking, not on whichever screen the compositor happens to list first.
fn monitor_at(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    let monitors = hypr_json(&["monitors", "-j"])?;
    let mut fallback = None;

    for monitor in monitors.as_array()? {
        let (Some(mx), Some(my), Some(width), Some(height)) = (
            monitor.get("x").and_then(Value::as_i64),
            monitor.get("y").and_then(Value::as_i64),
            monitor.get("width").and_then(Value::as_i64),
            monitor.get("height").and_then(Value::as_i64),
        ) else {
            continue;
        };

        let (mx, my, width, height) = (mx as i32, my as i32, width as i32, height as i32);
        if x >= mx && x < mx + width && y >= my && y < my + height {
            return Some((mx, my, width, height));
        }
        fallback = Some((mx, my, width, height));
    }

    fallback
}

/// Whether she can be dragged by the app itself. False off Hyprland, where the
/// native drag region is the only option and a click cannot be detected.
#[tauri::command]
pub fn drag_available() -> bool {
    crate::hypr::available()
}

/// Starts a drag: from here the window follows the pointer.
#[tauri::command]
pub fn drag_begin(app: AppHandle) -> Result<(), String> {
    let frame = active_window(&app).ok_or("could not read our own window")?;
    crate::hypr::begin(&frame.address, (frame.x, frame.y))
}

/// One step of a drag. Cheap: a socket round-trip, no process.
#[tauri::command]
pub fn drag_step() -> Result<(), String> {
    crate::hypr::step()
}

#[tauri::command]
pub fn drag_end() {
    crate::hypr::end();
}

/// Asks the compositor to keep both windows out of the tiling layout.
pub fn keep_floating() {
    for title in ["nerea-avatar", "nerea-chat"] {
        if let Some(frame) = own_window(title) {
            crate::hypr::float(&frame.address);
        }
    }
}

/// Parks her on the bottom-right corner of the focused window, whichever of her
/// two windows is showing.
///
/// Returns `None` on compositors without `hyprctl`; the overlay simply stays put.
#[tauri::command]
pub async fn dock(app: AppHandle) -> Result<Option<DockTarget>, String> {
    let Some(frame) = active_window(&app) else {
        return Ok(None);
    };

    let (width, height) = (frame.width, frame.height);
    let target = focused_client();

    let (mut x, mut y) = match target.as_ref() {
        Some(target) => (
            target.x + target.width - width + SEAT_OVERLAP,
            target.y + target.height - height + EDGE_INSET,
        ),
        None => (frame.x, frame.y),
    };

    let centre = (frame.x + frame.width / 2, frame.y + frame.height / 2);
    if let Some((mon_x, mon_y, mon_w, mon_h)) = monitor_at(centre.0, centre.1) {
        x = x.clamp(mon_x, mon_x + mon_w - width);
        y = y.clamp(mon_y, mon_y + mon_h - height);
    }

    place_window(&frame, x, y);

    Ok(target)
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

/// The last calls she made, for the history window.
///
/// One command reads the log so the page never parses a file and the user never
/// needs a terminal to see what she touched.
#[tauri::command]
pub async fn audit_calls() -> Result<Vec<nerea_core::tools::audit::Call>, String> {
    Ok(nerea_core::tools::audit::recent(50))
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
    crate::farewell_and_exit(app);
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

/// One proposal, shaped for the card that asks for approval.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingChange {
    id: String,
    path: String,
    kind: &'static str,
    at: u64,
    added: usize,
    removed: usize,
    preview: String,
}

/// What the page asks for once a second while a turn is busy.
#[tauri::command]
pub async fn pending_approval(
    state: tauri::State<'_, AppState>,
) -> Result<Option<serde_json::Value>, String> {
    Ok(state.approval_view())
}

/// His answer to a pending command. One id, one answer.
#[tauri::command]
pub async fn answer_approval(
    state: tauri::State<'_, AppState>,
    id: u64,
    approved: bool,
) -> Result<bool, String> {
    Ok(state.decide(id, approved))
}

/// Runs `run_command` for real: policy, then his click, then the process.
async fn run_command_tool(
    app: &AppHandle,
    state: &AppState,
    call: &nerea_core::ToolCall,
    budget: &mut nerea_core::tools::Budget,
) -> nerea_core::tools::ToolOutcome {
    let args: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let summary = nerea_core::tools::describe(call);

    let program = args.get("program").and_then(|value| value.as_str()).unwrap_or_default();
    let list: Vec<String> = args
        .get("args")
        .and_then(|value| value.as_array())
        .map(|items| items.iter().map(|item| item.as_str().unwrap_or_default().to_owned()).collect())
        .unwrap_or_default();
    let cwd = args.get("cwd").and_then(|value| value.as_str());

    let tools = nerea_core::exec::Tools::load();
    let planned = match nerea_core::exec::plan(&tools, program, &list, cwd) {
        Ok(plan) => plan,
        Err(err) => {
            nerea_core::tools::audit::append("run_command", &args, &summary, false, 0);
            return nerea_core::tools::charge(
                nerea_core::tools::ToolOutcome {
                    summary,
                    observation: format!("No puedo: {err}"),
                    ok: false,
                },
                budget,
            );
        }
    };

    // His click, unless a test asked for the shortcut.
    let approved = if std::env::var_os("NEREA_AUTO_APPROVE").is_some() {
        eprintln!("nerea exec: aprobación automática (NEREA_AUTO_APPROVE)");
        true
    } else {
        let (id, receiver) = state.request_approval();
        let view = serde_json::json!({
            "id": id,
            "program": planned.program,
            "args": planned.args,
            "cwd": planned.cwd.display().to_string(),
            "warning": nerea_core::exec::warning(&planned.program),
        });

        // Two ways to the same card: the event, and the poll the page runs while
        // it is busy. The event alone failed once too often.
        state.show_approval(Some(view.clone()));
        let _ = app.emit("nerea://panel", true);
        let _ = app.emit(APPROVE_EVENT, view);

        match tokio::time::timeout(std::time::Duration::from_secs(180), receiver).await {
            Ok(Ok(answer)) => answer,
            _ => false,
        }
    };

    state.show_approval(None);

    if !approved {
        let observation = format!("Lioner no aprobó `{}`: no lo ejecuté.", planned.program);
        nerea_core::tools::audit::append("run_command", &args, &summary, false, observation.len());
        return nerea_core::tools::charge(
            nerea_core::tools::ToolOutcome { summary, observation, ok: false },
            budget,
        );
    }

    let timeout = tools.timeout();
    let spawned =
        tokio::task::spawn_blocking(move || nerea_core::exec::run(&planned, timeout)).await;

    let (ok, observation) = match spawned {
        Ok(Ok(outcome)) => (outcome.ok(), outcome.report()),
        Ok(Err(err)) => (false, format!("No pude: {err}")),
        Err(err) => (false, format!("El proceso se perdió: {err}")),
    };

    nerea_core::tools::audit::append("run_command", &args, &summary, ok, observation.len());
    nerea_core::tools::charge(nerea_core::tools::ToolOutcome { summary, observation, ok }, budget)
}

/// `docs` is a network call, so it lives here and not in the sync tool table.
async fn docs_tool(
    call: &nerea_core::ToolCall,
    budget: &mut nerea_core::tools::Budget,
) -> nerea_core::tools::ToolOutcome {
    let args: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or_default();
    let summary = nerea_core::tools::describe(call);
    let library = args.get("library").and_then(|value| value.as_str()).unwrap_or_default();
    let question = args.get("question").and_then(|value| value.as_str()).unwrap_or_default();

    let (ok, observation) = match nerea_core::mcp::docs(library, question).await {
        Ok(text) => (true, text),
        Err(err) => (false, format!("No pude: {err}")),
    };

    nerea_core::tools::audit::append("docs", &args, &summary, ok, observation.len());
    nerea_core::tools::charge(nerea_core::tools::ToolOutcome { summary, observation, ok }, budget)
}

/// What she proposed and nobody approved yet. Read-only.
#[tauri::command]
pub async fn pending_changes() -> Result<Vec<PendingChange>, String> {
    Ok(nerea_core::changes::list()
        .into_iter()
        .map(|change| {
            let (added, removed) = change.counts();
            let preview = change.preview();
            PendingChange {
                id: change.id,
                path: change.path,
                kind: change.kind.label(),
                at: change.at,
                added,
                removed,
                preview,
            }
        })
        .collect())
}

/// The only path from a proposal to a real file, and it needs a human click.
#[tauri::command]
pub async fn apply_change(app: AppHandle, id: String) -> Result<String, String> {
    let said = nerea_core::changes::apply(&id)?;
    let _ = app.emit(REPLY_EVENT, serde_json::json!({ "text": said, "silent": true }));
    let _ = app.emit(PENDING_EVENT, true);
    Ok(said)
}

#[tauri::command]
pub async fn discard_change(app: AppHandle, id: String) -> Result<(), String> {
    nerea_core::changes::discard(&id)?;
    let _ = app.emit(PENDING_EVENT, true);
    Ok(())
}

/// Starts recording the microphone. The key press is the only caller.
#[tauri::command]
pub async fn listen_start(app: AppHandle) -> Result<(), String> {
    crate::hearing::start()?;
    let _ = app.emit(LISTEN_EVENT, ListenState { listening: true, error: None });
    Ok(())
}

/// Stops recording, transcribes, and hands the sentence to the reply path.
#[tauri::command]
pub async fn listen_stop(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    bank: tauri::State<'_, nerea_core::PhraseBank>,
) -> Result<String, String> {
    listen_turn(app, state, bank).await
}

/// One push-to-talk turn, from the released key to her answer.
pub async fn listen_turn(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    bank: tauri::State<'_, nerea_core::PhraseBank>,
) -> Result<String, String> {
    let _ = app.emit(LISTEN_EVENT, ListenState { listening: false, error: None });

    let audio = match crate::hearing::stop() {
        Ok(audio) => audio,
        Err(err) => {
            // "muy corto" and "silencio" are not failures to hide: without a word
            // she would look like she ignored him.
            let _ = app.emit(
                LISTEN_EVENT,
                ListenState { listening: false, error: Some(err.clone()) },
            );
            return Err(err);
        }
    };

    let heard = crate::hearing::transcribe(audio).await?;
    eprintln!("nerea ear: oído «{heard}»");
    let Some(text) = nerea_core::hearing::clean(&heard) else {
        let _ = app.emit(
            LISTEN_EVENT,
            ListenState { listening: false, error: Some("no te escuché".to_owned()) },
        );
        return Err("no te escuché".to_owned());
    };

    // The reply path is the same one the chat uses, but the chat never asked
    // for it: without this event his words and her answer exist everywhere
    // except on screen.
    let _ = app.emit(HEARD_EVENT, serde_json::json!({ "text": text }));

    send_message(app.clone(), state, bank, text.clone()).await?;
    Ok(text)
}

/// The reply, said in Japanese: one small call, faithful to the Spanish text.
pub(crate) async fn japanese_line(
    provider: &std::sync::Arc<dyn nerea_core::AiProvider>,
    reply: &str,
) -> Option<String> {
    const PROMPT: &str = "Traducí al japonés hablado, natural y breve, esta respuesta de Nerea. \
Máximo 60 caracteres. Reglas: llamá al usuario 私の竜 o 愛しの竜, nunca ドラゴン; si dice «vaya, vaya» o «anda, \
anda», en japonés eso es あらあら; no agregues nada que no esté en la respuesta. Devolvé solo la línea: sin comillas, \
sin romaji, sin explicación.";

    let request = nerea_core::ChatRequest::new(
        vec![
            nerea_core::ChatMessage::system(PROMPT),
            nerea_core::ChatMessage::user(reply.chars().take(1_000).collect::<String>()),
        ],
        "voice",
    );

    let answer = provider.chat_completion(&request).await.ok()?;
    let line = answer.content.trim().trim_matches('"').trim().to_owned();

    (!line.is_empty()).then_some(line)
}

/// One short extra call: the tone of the reply decides which reaction plays.
async fn mood(provider: &std::sync::Arc<dyn nerea_core::AiProvider>, reply: &str) -> Option<&'static str> {
    const PROMPT: &str = "Clasificá el tono de la respuesta de un asistente en UNA sola palabra. \
Opciones: neutral, bromista, carinosa, sorprendida. Respondé solo la palabra, sin explicar.";

    let request = nerea_core::ChatRequest::new(
        vec![
            nerea_core::ChatMessage::system(PROMPT),
            nerea_core::ChatMessage::user(reply.chars().take(600).collect::<String>()),
        ],
        "mood",
    );

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
/// Says goodbye once, and reports how long the line lasts.
pub(crate) fn farewell(app: &AppHandle) -> Option<u64> {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SAID.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return None;
    }

    let bank = app.try_state::<PhraseBank>()?;
    speak(app, &bank, "goodbye").map(|said| said.duration_ms)
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

/// While she reads, the subtitle says what she is doing.
const TOOL_EVENT: &str = "nerea://tool";
const LISTEN_EVENT: &str = "nerea://listen";
/// What the microphone understood, so the chat can show his side of it.
const HEARD_EVENT: &str = "nerea://heard";
/// The finished reply, whether it came from a keyboard or from the microphone.
const REPLY_EVENT: &str = "nerea://reply";
/// Something was proposed, applied or discarded: the cards refresh.
const PENDING_EVENT: &str = "nerea://pending";
/// A command is waiting for his click.
const APPROVE_EVENT: &str = "nerea://approve";

/// What the microphone is doing, and why it stopped when it stopped badly.
#[derive(Debug, Clone, serde::Serialize)]
struct ListenState {
    listening: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ToolNote {
    text: String,
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
    let mut request = turn.request;

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

    let mut reply = String::new();
    let mut cancelled = false;
    let mut budget = nerea_core::tools::Budget::per_message();

    // Reading is a loop: she asks for a file, the app runs the tool, and she
    // answers with the result in hand. Bounded, because each look is another
    // round trip and another slice of the context.
    //
    // The last round ignores any call it still asks for: a turn may not end on a
    // tool round. Restricting the tools instead is worse — this model answers by
    // writing its own call syntax into the prose, and that markup reaches the
    // chat (measured; see AGENTS.md).
    for round in 0..=nerea_core::tools::MAX_ROUNDS {
        let answering = round == nerea_core::tools::MAX_ROUNDS;
        let said_before = reply.chars().count();

        let streamed = match stream_reply(&app, state.inner(), &provider, &request, &mut reply).await
        {
            Ok(streamed) => streamed,
            Err(err) => {
                // The reason belongs in the log: a turn that ends mute is not
                // debuggable from the outside.
                eprintln!("nerea reply: {err}");
                state.finish_turn(&reply).await;
                emit_state(&app, "error");
                speak(&app, &bank, "complain");
                return Err(err);
            }
        };

        cancelled = streamed.cancelled;
        if cancelled || answering || streamed.calls.is_empty() {
            break;
        }

        let said: String = reply.chars().skip(said_before).collect();

        let mut calls = Vec::new();
        let mut results = Vec::new();
        for call in streamed.calls.into_iter().take(nerea_core::tools::MAX_CALLS_PER_ROUND) {
            let _ = app.emit(TOOL_EVENT, ToolNote { text: nerea_core::tools::describe(&call) });
            let outcome = match call.name.as_str() {
                "run_command" => run_command_tool(&app, state.inner(), &call, &mut budget).await,
                "docs" => docs_tool(&call, &mut budget).await,
                _ => nerea_core::tools::run_charged(&call, &mut budget),
            };
            results.push(outcome.observation);
            calls.push(call);
        }
        state.push_tool_round(&said, calls, results).await;

        // Her next round is a new paragraph, not a continuation of the line she
        // said before looking.
        if !reply.is_empty() && !reply.ends_with('\n') {
            let _ = app.emit(DELTA_EVENT, Delta { text: "\n\n".to_owned() });
            reply.push_str("\n\n");
        }

        request = state.tool_request().await;
    }

    // A turn never ends silent. If every round came back empty, say so: a mute
    // reply with files already read is worse than admitting it.
    if reply.trim().is_empty() && !cancelled {
        let line = "Me quedé en blanco y no te sirve. Preguntame otra vez, dale.";
        let _ = app.emit(DELTA_EVENT, Delta { text: line.to_owned() });
        reply.push_str(line);
    }

    let turns = state.finish_turn(&reply).await;
    let _ = app.emit(REPLY_EVENT, serde_json::json!({ "text": reply }));
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

struct Streamed {
    calls: Vec<nerea_core::ToolCall>,
    cancelled: bool,
}

/// One model turn: her words as they are written, and the calls she asked for.
///
/// Streaming and tools coexist without a protocol in the prompt because the API
/// reports the calls in their own field. Nothing has to be parsed out of prose,
/// and nothing can be half-said.
async fn stream_reply(
    app: &AppHandle,
    state: &AppState,
    provider: &Arc<dyn AiProvider>,
    request: &ChatRequest,
    reply: &mut String,
) -> Result<Streamed, String> {
    use futures::StreamExt;

    let mut stream =
        provider.stream_chat_completion(request).await.map_err(|err| err.to_string())?;

    emit_state(app, "thinking");

    let mut talking = false;
    let mut cancelled = false;
    let mut calls = Vec::new();

    while let Some(item) = stream.next().await {
        if state.take_cancel() {
            cancelled = true;
            break;
        }

        let item = item.map_err(|err| err.to_string())?;
        if !item.content.is_empty() {
            show(app, reply, &item.content, &mut talking);
        }
        calls.extend(item.tool_calls);
    }

    Ok(Streamed {
        calls: if cancelled { Vec::new() } else { calls },
        cancelled,
    })
}

fn show(app: &AppHandle, reply: &mut String, text: &str, talking: &mut bool) {
    if text.is_empty() {
        return;
    }
    if !*talking {
        *talking = true;
        emit_state(app, "talking");
    }
    reply.push_str(text);
    let _ = app.emit(DELTA_EVENT, Delta { text: text.to_owned() });
}
