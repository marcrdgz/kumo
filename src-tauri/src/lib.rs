mod commands;
mod config;
mod debug;
mod editor;
mod pty;
mod session;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(session::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::create_session,
            commands::split_pane,
            commands::attach_pane,
            commands::list_sessions,
            commands::get_session,
            commands::write_pane,
            commands::resize_pane,
            commands::focus_pane,
            commands::close_pane,
            commands::close_session,
            commands::default_shell_command,
            commands::open_ai_pane,
            commands::ai_command,
            commands::ai_command_line,
            commands::editor_context,
            commands::pane_cwd,
            commands::pane_shell,
            commands::save_layout,
            commands::load_layout,
            commands::debug_log,
        ])
        .setup(|app| {
            let state: tauri::State<session::AppState> = app.state();
            let _ = state;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running neomux");
}
