mod commands;
mod control;
mod engine;
mod hearing;
mod hypr;
mod state;
mod update;
mod voice;

use tauri::{Emitter, Manager};

/// Closing her is closing the app, but never mid-sentence: she says goodbye, the
/// line stays long enough to be read, and only then does the process leave.
pub fn farewell_and_exit(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let hold = commands::farewell(&app).unwrap_or(1400) + 900;
        tokio::time::sleep(std::time::Duration::from_millis(hold)).await;
        engine::stop();
        hearing::stop_engine();
        app.exit(0);
    });
}

pub fn run() {
    let built = tauri::Builder::default()
        .manage(state::AppState::new())
        .manage(nerea_core::PhraseBank::load().unwrap_or_default())
        .setup(|app| {

            // Development aid: open the panel without a pointer so its layout can
            // be checked from a script. Never set in normal use.
            // She resizes with the chat, which costs her the fixed-size hint that
            // made the compositor float her by itself.
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                commands::keep_floating();
            });

            // Idle reaper: the engine is expensive and should not sit there all day.
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    engine::reap();
                    hearing::reap();
                }
            });

            // Development aid: say one sentence through the whole voice path.
            if let Ok(text) = std::env::var("NEREA_SAY_TEST") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    std::thread::sleep(std::time::Duration::from_millis(1200));
                    if let Err(err) = engine::ensure().await {
                        eprintln!("nerea voice: engine unavailable: {err}");
                        return;
                    }
                    eprintln!("nerea voice: engine {:?}", engine::state());

                    let state = handle.state::<state::AppState>().inner().clone();
                    let Ok(provider) = state.ensure_provider().await else {
                        eprintln!("nerea voice: no provider");
                        return;
                    };
                    let Some(japanese) = commands::japanese_line(&provider, &text).await else {
                        eprintln!("nerea voice: no japanese line");
                        return;
                    };
                    eprintln!("nerea voice: ja={japanese}");

                    let Some(reference) = handle
                        .state::<nerea_core::PhraseBank>()
                        .reference_prompt()
                        .map(str::to_owned)
                    else {
                        eprintln!("nerea voice: no reference prompt");
                        return;
                    };

                    match voice::speak(&japanese, &text, &reference).await {
                        Ok(line) => {
                            eprintln!("nerea voice: {} ms of audio", line.duration_ms);
                            let _ = handle.emit("nerea://say", &line);
                        }
                        Err(err) => eprintln!("nerea voice: failed: {err}"),
                    }
                });
            }

            // Development aid: print what recall would send for a query.
            if let Ok(query) = std::env::var("NEREA_RECALL_QUERY") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    std::thread::sleep(std::time::Duration::from_millis(900));
                    let state = handle.state::<state::AppState>().inner().clone();
                    match state.recall_preview(&query).await {
                        Some(block) => eprintln!("nerea recall:\n{block}"),
                        None => eprintln!("nerea recall: nothing found"),
                    }
                });
            }

            // Development aid: print the cards a message would carry.
            if let Ok(query) = std::env::var("NEREA_CARDS_QUERY") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    std::thread::sleep(std::time::Duration::from_millis(900));
                    let state = handle.state::<state::AppState>().inner().clone();
                    match state.cards_preview(&query).await {
                        Some(block) => eprintln!("nerea cards:\n{block}"),
                        None => eprintln!("nerea cards: nothing selected"),
                    }
                });
            }

            // Development aid: force the summarizer once, to check it without
            // waiting for two dozen turns.
            if std::env::var_os("NEREA_SUMMARIZE_NOW").is_some() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    std::thread::sleep(std::time::Duration::from_millis(1200));
                    let state = handle.state::<state::AppState>().inner().clone();
                    match state.ensure_provider().await {
                        Ok(provider) => match state.summarize_if_due(&provider).await {
                            Some(text) => eprintln!("nerea memory: {}", text.chars().take(160).collect::<String>()),
                            None => eprintln!("nerea memory: nothing to summarize yet"),
                        },
                        Err(err) => eprintln!("nerea memory: no provider: {err}"),
                    }
                });
            }

            // Development aid: run the same check the menu runs and, when asked,
            // apply it. Off unless the environment says otherwise.
            if std::env::var_os("NEREA_AUTO_UPDATE").is_some() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let status = update::status();
                    eprintln!(
                        "nerea update: behind={} dirty={} can={}",
                        status.behind, status.dirty, status.can_update
                    );
                    if !status.can_update {
                        return;
                    }
                    match update::pull().and_then(|_| update::build()) {
                        Ok(()) => {
                            eprintln!("nerea update: rebuilt, restarting");
                            handle.restart();
                        }
                        Err(err) => eprintln!("nerea update: failed: {err}"),
                    }
                });
            }

            if std::env::var_os("NEREA_OPEN_HISTORY").is_some() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                    let _ = commands::open_history(handle).await;
                });
            }

            // Sends one message by itself: the only way to exercise the whole
            // reply path (tools included) without a pointer.
            // The way in from outside: a named pipe, because signals in a
            // process that embeds WebKit end in a SIGSEGV inside JavaScriptCore.
            control::start(app.handle().clone());

            // Development aid: one push-to-talk turn by itself, so the whole
            // path runs without a key.
            if let Ok(ms) = std::env::var("NEREA_LISTEN_MS") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let ms: u64 = ms.parse().unwrap_or(3000);
                    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                    if let Err(err) = commands::listen_start(handle.clone()).await {
                        eprintln!("nerea ear: {err}");
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;

                    let state = handle.state::<state::AppState>();
                    let bank = handle.state::<nerea_core::PhraseBank>();
                    match commands::listen_turn(handle.clone(), state, bank).await {
                        Ok(text) => eprintln!("nerea ear: escuché «{text}»"),
                        Err(err) => eprintln!("nerea ear: {err}"),
                    }
                });
            }

            if let Ok(ask) = std::env::var("NEREA_ASK") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
                    let state = handle.state::<crate::state::AppState>();
                    let bank = handle.state::<nerea_core::PhraseBank>();
                    let _ = commands::send_message(handle.clone(), state, bank, ask).await;
                });
            }

            if std::env::var_os("NEREA_OPEN_PANEL").is_some() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                    let _ = handle.emit("nerea://panel", true);
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::status,
            commands::send_message,
            commands::cancel_stream,
            commands::reset_conversation,
            commands::dock,
            commands::drag_available,
            commands::drag_begin,
            commands::drag_step,
            commands::drag_end,
            commands::greet,
            commands::say,
            commands::say_categories,
            commands::open_panel,
            commands::close_panel,
            commands::open_history,
            commands::close_history,
            commands::history,
            commands::audit_calls,
            commands::transcript,
            commands::quit,
            commands::update_status,
            commands::update_apply,
            commands::voice_state,
            commands::voice_preference,
            commands::voice_set_preference,
            commands::listen_start,
            commands::listen_stop,
        ])
        .build(tauri::generate_context!());

    let app = match built {
        Ok(app) => app,
        Err(err) => {
            eprintln!("nerea: could not start the window: {err}");
            std::process::exit(1);
        }
    };

    app.run(|handle, event| {
        if std::env::var_os("NEREA_DEBUG").is_some() {
            eprintln!("nerea event: {event:?}");
        }
        match event {
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => match label.as_str() {
                // Closing her is closing the app, even with the history hidden.
                "avatar" => {
                    api.prevent_close();
                    farewell_and_exit(handle.clone());
                }
                // The history window is a view, not a quittable app window.
                "history" => {
                    api.prevent_close();
                    if let Some(window) = handle.get_webview_window("history") {
                        let _ = window.hide();
                    }
                    if let Some(avatar) = handle.get_webview_window("avatar") {
                        let _ = avatar.show();
                    }
                }
                _ => {}
            },
            tauri::RunEvent::ExitRequested { .. } => {
                engine::stop();
                commands::farewell(handle);
            }
            _ => {}
        }
    });
}
