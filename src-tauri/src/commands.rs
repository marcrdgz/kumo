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
            request.ai,
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

/// Detect the active process in a pane and return a short dynamic title
/// (e.g. `vim: main.rs` or `server.js`). `None` when the shell is idle.
#[tauri::command]
pub fn pane_title(state: State<AppState>, request: PaneRequest) -> Result<Option<String>, String> {
    let pid = state
        .pane_child_pid(request.session_id, request.pane_id)
        .map_err(|e| e.to_string())?;
    Ok(crate::editor::pane_title(pid))
}

/// Return the git status of the current workspace (branch + changes).
#[tauri::command]
pub fn git_status() -> Option<crate::git::GitStatus> {
    let ws = get_workspace().ok().flatten()?;
    let dir = std::path::PathBuf::from(ws);
    if !dir.is_dir() {
        return None;
    }
    crate::git::status(&dir)
}

/// Return the unified diff for one file in the current workspace.
#[tauri::command]
pub fn git_diff(path: String) -> Result<String, String> {
    let ws = get_workspace().ok().flatten().ok_or_else(|| "no workspace".to_string())?;
    let dir = std::path::PathBuf::from(ws);
    if !dir.is_dir() {
        return Err("workspace is not a directory".to_string());
    }
    Ok(crate::git::diff(&dir, &path))
}

fn neomux_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::PathBuf::from(home).join(".neomux")
}

/// Stable per-workspace layout path: `~/.neomux/layouts/<hash>/layout.json`.
/// Hash is derived from the workspace path so each folder keeps its own grid.
fn layout_path() -> std::path::PathBuf {
    let current = get_workspace().ok().flatten();
    let dir = match current {
        Some(ws) => neomux_dir().join("layouts").join(&workspace_hash(&ws)),
        None => neomux_dir(),
    };
    dir.join("layout.json")
}

/// Deterministic short hash of a path (FNV-1a, hex) for subdirectory names.
fn workspace_hash(path: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in path.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

fn workspace_path() -> std::path::PathBuf {
    neomux_dir().join("workspace.json")
}

fn recent_path() -> std::path::PathBuf {
    neomux_dir().join("workspaces.json")
}

/// Persist the active workspace folder (~/.neomux/workspace.json) and push it
/// to the front of the recent list (~/.neomux/workspaces.json, max 5). All
/// panes and the AI pane spawn relative to it so `@file` references resolve.
#[tauri::command]
pub fn set_workspace(app: AppHandle, path: String) -> Result<(), String> {
    write_workspace(&path)?;
    if let Ok(menu) = crate::menu::build_menu(&app) {
        let _ = app.set_menu(menu);
    }
    Ok(())
}

/// Persist the workspace and update the recent list. Shared by the command and
/// tests (the command additionally rebuilds the native menu).
pub(crate) fn write_workspace(path: &str) -> Result<(), String> {
    let path_buf = std::path::PathBuf::from(path);
    if !path_buf.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    std::fs::create_dir_all(neomux_dir()).map_err(|e| e.to_string())?;
    std::fs::write(workspace_path(), path).map_err(|e| e.to_string())?;

    let mut recents = read_recents();
    recents.retain(|w| w != path);
    recents.insert(0, path.to_string());
    recents.truncate(5);
    std::fs::write(recent_path(), serde_json::to_string(&recents).unwrap_or_else(|_| "[]".into()))
        .map_err(|e| e.to_string())
}

/// Return the persisted workspace folder, if any.
#[tauri::command]
pub fn get_workspace() -> Result<Option<String>, String> {
    match std::fs::read_to_string(workspace_path()) {
        Ok(s) => Ok(Some(s)),
        Err(_) => Ok(None),
    }
}

/// Return the most recently opened workspaces (most recent first, max 5).
#[tauri::command]
pub fn get_recent_workspaces() -> Result<Vec<String>, String> {
    Ok(read_recents())
}

/// Read the recent-workspaces list, tolerating a missing/corrupt file.
pub(crate) fn read_recents() -> Vec<String> {
    match std::fs::read_to_string(recent_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// Persist the session layout (JSON) to the current workspace's layout dir.
#[tauri::command]
pub fn save_layout(layout: String) -> Result<(), String> {
    let path = layout_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, layout).map_err(|e| e.to_string())
}

/// Load a previously persisted session layout for the current workspace, if any.
#[tauri::command]
pub fn load_layout() -> Result<Option<String>, String> {
    match std::fs::read_to_string(layout_path()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_hash_is_deterministic() {
        let a = workspace_hash("/Users/x/project");
        let b = workspace_hash("/Users/x/project");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn workspace_hash_differs_by_path() {
        let a = workspace_hash("/Users/x/project-a");
        let b = workspace_hash("/Users/x/project-b");
        assert_ne!(a, b);
    }

    #[test]
    fn recents_keeps_most_recent_first_and_caps() {
        let mut recents = vec!["b".to_string(), "c".to_string()];
        let path = "a".to_string();
        recents.retain(|w| w != &path);
        recents.insert(0, path);
        recents.truncate(5);
        assert_eq!(recents, vec!["a", "b", "c"]);

        // Full list, new item pushes oldest out.
        let mut full: Vec<String> = vec!["1", "2", "3", "4", "5"].into_iter().map(String::from).collect();
        let path = "0".to_string();
        full.retain(|w| w != &path);
        full.insert(0, path);
        full.truncate(5);
        assert_eq!(full, vec!["0", "1", "2", "3", "4"]);
    }

    #[test]
    fn recents_moves_existing_to_front() {
        let mut recents = vec!["a".to_string(), "b".to_string()];
        let path = "a".to_string();
        recents.retain(|w| w != &path);
        recents.insert(0, path);
        assert_eq!(recents, vec!["a", "b"]);
    }

    #[test]
    fn layout_path_is_per_workspace() {
        // Redirect HOME to a temp dir so we don't touch the real config, and
        // restore it afterwards since other tests share the process env.
        let orig_home = std::env::var("HOME").ok();
        let tmp = std::env::temp_dir().join(format!("neomux-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        std::env::set_var("HOME", &tmp);

        // No workspace set -> legacy flat path.
        let _ = std::fs::remove_file(workspace_path());
        let flat = layout_path();
        assert_eq!(flat.file_name().unwrap(), "layout.json");
        assert_eq!(flat.parent().unwrap().file_name().unwrap(), ".neomux");

        // Real dirs so set_workspace's is_dir() check passes.
        let ws_a = tmp.join("project-a");
        let ws_b = tmp.join("project-b");
        std::fs::create_dir_all(&ws_a).unwrap();
        std::fs::create_dir_all(&ws_b).unwrap();

        // Workspace set -> per-folder hash dir.
        write_workspace(&ws_a.to_string_lossy()).unwrap();
        let per = layout_path();
        assert_eq!(per.file_name().unwrap(), "layout.json");
        let hash_dir = per.parent().unwrap();
        let expected_hash = workspace_hash(&ws_a.to_string_lossy());
        assert_eq!(hash_dir.file_name().unwrap().to_string_lossy(), expected_hash);

        // Different workspace -> different dir.
        write_workspace(&ws_b.to_string_lossy()).unwrap();
        let per_b = layout_path();
        assert_ne!(per_b, per);

        // Same workspace again -> same dir (stable).
        write_workspace(&ws_a.to_string_lossy()).unwrap();
        assert_eq!(layout_path(), per);

        let _ = std::fs::remove_dir_all(&tmp);
        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}
