mod commands;
mod state;


pub fn run() {
    let built = tauri::Builder::default()
        .manage(state::AppState::new())
        .manage(nerea_core::PhraseBank::load().unwrap_or_default())
        .invoke_handler(tauri::generate_handler![
            commands::status,
            commands::send_message,
            commands::cancel_stream,
            commands::reset_conversation,
            commands::dock,
            commands::say,
            commands::say_categories,
            commands::open_composer,
            commands::close_composer,
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
            // Any window closing is "I am leaving"; the guard inside farewell keeps
            // it to one line even if both windows close.
            tauri::RunEvent::WindowEvent {
                event: tauri::WindowEvent::CloseRequested { .. },
                ..
            }
            | tauri::RunEvent::ExitRequested { .. } => commands::farewell(handle),
            _ => {}
        }
    });
}
