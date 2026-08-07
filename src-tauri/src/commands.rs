use tauri::{AppHandle, Emitter, State};

use crate::session::{AppState, PaneRequest, ResizeRequest, SessionInfo, SpawnRequest, SplitRequest};

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PaneOutput {
    session_id: u64,
    pane_id: u64,
    data: String, // base64-encoded chunk
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PaneClosed {
    session_id: u64,
    pane_id: u64,
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// Start a new session and spawn the first pane.
#[tauri::command]
pub fn create_session(
    _app: AppHandle,
    state: State<AppState>,
    request: SpawnRequest,
) -> Result<SessionInfo, String> {
    let shell = request.shell.filter(|s| !s.is_empty()).unwrap_or_else(default_shell);
    state
        .create_session(
            request.name.as_deref().unwrap_or(""),
            &shell,
            request.cwd.as_deref(),
            request.cols,
            request.rows,
        )
        .map_err(|e| e.to_string())
}

/// Create a new pane (split) in an existing session.
#[tauri::command]
pub fn split_pane(
    _app: AppHandle,
    state: State<AppState>,
    request: SplitRequest,
) -> Result<crate::session::PaneInfo, String> {
    let program = request.program.map(|p| (p, request.args.unwrap_or_default()));
    let shell = request
        .shell
        .filter(|s| !s.is_empty())
        .unwrap_or_else(default_shell);
    state
        .split_pane(
            request.session_id,
            &shell,
            program,
            request.cwd.as_deref(),
            request.cols,
            request.rows,
            &request.direction,
        )
        .map_err(|e| e.to_string())
}

/// Start streaming pane output to the frontend. Called by the UI once the
/// terminal element exists, so no early output (e.g. DA queries) is lost.
#[tauri::command]
pub fn attach_pane(app: AppHandle, state: State<AppState>, request: PaneRequest) -> Result<(), String> {
    attach_read_loop(&app, &state, request.session_id, request.pane_id);
    Ok(())
}

#[tauri::command]
pub fn list_sessions(state: State<AppState>) -> Vec<SessionInfo> {
    state.list_sessions()
}

#[tauri::command]
pub fn get_session(state: State<AppState>, session_id: u64) -> Result<SessionInfo, String> {
    state.get_session(session_id).ok_or_else(|| "session not found".into())
}

#[tauri::command]
pub fn write_pane(
    state: State<AppState>,
    request: PaneRequest,
    data: String,
) -> Result<(), String> {
    state
        .write_pane(request.session_id, request.pane_id, data.as_bytes())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn resize_pane(state: State<AppState>, request: ResizeRequest) -> Result<(), String> {
    state
        .resize_pane(request.session_id, request.pane_id, request.cols, request.rows)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn focus_pane(state: State<AppState>, request: PaneRequest) -> Result<(), String> {
    state
        .focus_pane(request.session_id, request.pane_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn close_pane(
    app: AppHandle,
    state: State<AppState>,
    request: PaneRequest,
) -> Result<bool, String> {
    let removed_session = state
        .close_pane(request.session_id, request.pane_id)
        .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "pane-closed",
        PaneClosed {
            session_id: request.session_id,
            pane_id: request.pane_id,
        },
    );
    Ok(removed_session)
}

#[tauri::command]
pub fn close_session(state: State<AppState>, session_id: u64) -> Result<(), String> {
    state.close_session(session_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn default_shell_command() -> String {
    default_shell()
}

/// Open an AI pane in the given session: splits the active pane and spawns
/// the configured AI CLI inside it.
#[tauri::command]
pub fn open_ai_pane(
    state: State<AppState>,
    request: crate::session::AiPaneRequest,
) -> Result<crate::session::PaneInfo, String> {
    let (program, args) = crate::config::ai_command();
    let cwd = crate::config::ai_cwd();
    state
        .spawn_ai_pane(
            request.session_id,
            &program,
            &args,
            cwd,
            request.cols,
            request.rows,
        )
        .map_err(|e| e.to_string())
}

/// Return the resolved AI CLI command (program only) for display in the UI.
#[tauri::command]
pub fn ai_command() -> String {
    crate::config::ai_command().0
}

/// Return the full AI CLI command line (program + args) for restoring AI panes.
#[tauri::command]
pub fn ai_command_line() -> (String, Vec<String>) {
    crate::config::ai_command()
}

/// Detect the editor (vim/nvim) running in a pane and return the file it is
/// editing. Used to send `@file:line:col` references to the AI pane.
#[tauri::command]
pub fn editor_context(
    state: State<AppState>,
    request: PaneRequest,
) -> Result<Option<crate::editor::EditorContext>, String> {
    let pid = state
        .pane_child_pid(request.session_id, request.pane_id)
        .map_err(|e| e.to_string())?;
    Ok(crate::editor::editor_context(pid))
}

/// Append a diagnostics line to /tmp/neomux_debug.log (dev aid).
#[tauri::command]
pub fn debug_log(msg: String) {
    crate::debug::log(&msg);
}

/// Return the working directory of a pane's child process (best effort).
#[tauri::command]
pub fn pane_cwd(state: State<AppState>, request: PaneRequest) -> Result<Option<String>, String> {
    let pid = state
        .pane_child_pid(request.session_id, request.pane_id)
        .map_err(|e| e.to_string())?;
    Ok(crate::editor::process_cwd(pid).map(|p| p.to_string_lossy().to_string()))
}

/// Return the shell/program used to spawn a pane.
#[tauri::command]
pub fn pane_shell(state: State<AppState>, request: PaneRequest) -> Result<String, String> {
    state
        .pane_shell(request.session_id, request.pane_id)
        .map_err(|e| e.to_string())
}

fn layout_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join(".neomux").join("layout.json")
}

/// Persist the session layout (JSON) to ~/.neomux/layout.json.
#[tauri::command]
pub fn save_layout(layout: String) -> Result<(), String> {
    let path = layout_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, layout).map_err(|e| e.to_string())
}

/// Load a previously persisted session layout, if any.
#[tauri::command]
pub fn load_layout() -> Result<Option<String>, String> {
    let path = layout_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

/// Start reading output from a pane and emitting it to the frontend as
/// `pane-output` events.
fn attach_read_loop(app: &AppHandle, state: &AppState, session_id: u64, pane_id: u64) {
    let app = app.clone();
    let _ = state.detach_read_loop(session_id, pane_id, move |pane_id, data| {
        let _ = app.emit(
            "pane-output",
            PaneOutput {
                session_id,
                pane_id,
                data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data),
            },
        );
    });
}
