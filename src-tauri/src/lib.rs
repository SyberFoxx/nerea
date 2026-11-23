mod commands;
mod state;

use tauri::{Emitter, Manager};

pub fn run() {
    let built = tauri::Builder::default()
        .manage(state::AppState::new())
        .manage(nerea_core::PhraseBank::load().unwrap_or_default())
        .setup(|app| {

            // Development aid: open the panel without a pointer so its layout can
            // be checked from a script. Never set in normal use.
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
                    api.prevent_close();
                    handle.exit(0);
                }
                // The history window is a view, not a quittable app window.
                "history" => {
                    api.prevent_close();
                    if let Some(window) = handle.get_webview_window("history") {
                        let _ = window.hide();
                    }
                }
                _ => {}
            },
            tauri::RunEvent::ExitRequested { .. } => commands::farewell(handle),
            _ => {}
        }
    });
}
