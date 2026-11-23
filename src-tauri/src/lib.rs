mod commands;
mod engine;
mod hypr;
mod state;
mod update;
mod voice;

use tauri::{Emitter, Manager};

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
            commands::transcript,
            commands::quit,
            commands::update_status,
            commands::update_apply,
            commands::voice_state,
            commands::voice_preference,
            commands::voice_set_preference,
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
                    commands::farewell(handle);
                    engine::stop();
                    api.prevent_close();
                    handle.exit(0);
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
