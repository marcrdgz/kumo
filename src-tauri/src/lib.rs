mod commands;
mod config;
mod debug;
mod editor;
mod menu;
mod pty;
mod session;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
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
            commands::pane_title,
            commands::save_layout,
            commands::load_layout,
            commands::set_workspace,
            commands::get_workspace,
            commands::get_recent_workspaces,
            commands::debug_log,
        ])
        .menu(|handle| menu::build_menu(handle))
        .on_menu_event(menu::handle_menu_event)
        .setup(|app| {
            let state: tauri::State<session::AppState> = app.state();
            let _ = state;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running neomux");
}
