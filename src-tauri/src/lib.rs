mod commands;
mod state;

pub fn run() {
    let started = tauri::Builder::default()
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::status,
            commands::send_message,
            commands::cancel_stream,
            commands::reset_conversation,
            commands::dock,
            commands::open_composer,
            commands::close_composer,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = started {
        eprintln!("nerea: could not start the window: {err}");
        std::process::exit(1);
    }
}
