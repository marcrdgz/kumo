use tauri::menu::{AboutMetadata, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Runtime};

/// Build the native macOS application menu. "Open Recent" is populated from the
/// persisted recent-workspaces list, so it needs a rebuild whenever that changes.
pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let app_menu = Submenu::with_items(
        app,
        "neomux",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About neomux"), Some(AboutMetadata::default()))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some("Hide neomux"))?,
            &PredefinedMenuItem::hide_others(app, Some("Hide Others"))?,
            &PredefinedMenuItem::show_all(app, Some("Show All"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some("Quit neomux"))?,
        ],
    )?;

    let open_folder = MenuItem::with_id(app, "file.open_folder", "Open Folder…", true, Some("CmdOrCtrl+O"))?;
    let close_window = PredefinedMenuItem::close_window(app, Some("Close Window"))?;
    let recent_items = build_recent_items(app)?;
    let recent_refs: Vec<&dyn IsMenuItem<R>> = recent_items.iter().map(|i| i.as_ref()).collect();
    let open_recent = Submenu::with_items(app, "Open Recent", true, recent_refs.as_slice())?;
    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &open_folder,
            &open_recent,
            &PredefinedMenuItem::separator(app)?,
            &close_window,
        ],
    )?;

    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, Some("Undo"))?,
            &PredefinedMenuItem::redo(app, Some("Redo"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, Some("Cut"))?,
            &PredefinedMenuItem::copy(app, Some("Copy"))?,
            &PredefinedMenuItem::paste(app, Some("Paste"))?,
            &PredefinedMenuItem::select_all(app, Some("Select All"))?,
        ],
    )?;

    let split_h = MenuItem::with_id(app, "view.split_h", "Split Horizontal", true, Some("CmdOrCtrl+Shift+H"))?;
    let split_v = MenuItem::with_id(app, "view.split_v", "Split Vertical", true, Some("CmdOrCtrl+Shift+V"))?;
    let zoom = MenuItem::with_id(app, "view.zoom", "Zoom Pane", true, Some("CmdOrCtrl+Shift+Z"))?;
    let close_pane = MenuItem::with_id(app, "view.close_pane", "Close Pane", true, Some("CmdOrCtrl+Shift+W"))?;
    let search = MenuItem::with_id(app, "view.search", "Search Scrollback", true, Some("CmdOrCtrl+F"))?;
    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[&split_h, &split_v, &zoom, &close_pane, &search],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, Some("Minimize"))?,
            &PredefinedMenuItem::maximize(app, Some("Zoom"))?,
        ],
    )?;

    Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu])
}

/// Recent-workspace items, most recent first (mirrors the welcome screen list).
fn build_recent_items<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Vec<Box<dyn IsMenuItem<R>>>> {
    let mut items: Vec<Box<dyn IsMenuItem<R>>> = Vec::new();
    let recents = super::commands::read_recents();
    if recents.is_empty() {
        items.push(Box::new(
            MenuItem::with_id(app, "file.open_recent.empty", "No Recent Folders", false, None::<&str>)?,
        ));
        return Ok(items);
    }
    for path in recents {
        let name = path
            .split('/')
            .filter(|s| !s.is_empty())
            .next_back()
            .unwrap_or(&path);
        items.push(Box::new(MenuItem::with_id(
            app,
            format!("file.open_recent:{path}"),
            name,
            true,
            None::<&str>,
        )?));
    }
    Ok(items)
}

/// Route a menu click to the frontend via a `menu-*` event. The webview listens
/// for these and performs the matching action (open dialog, split, etc.).
pub fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    let id = event.id().as_ref();
    match id {
        "file.open_folder" => {
            let _ = app.emit("menu-open-folder", ());
        }
        "view.split_h" => {
            let _ = app.emit("menu-split-h", ());
        }
        "view.split_v" => {
            let _ = app.emit("menu-split-v", ());
        }
        "view.zoom" => {
            let _ = app.emit("menu-zoom", ());
        }
        "view.close_pane" => {
            let _ = app.emit("menu-close-pane", ());
        }
        "view.search" => {
            let _ = app.emit("menu-search", ());
        }
        _ => {
            if let Some(path) = id.strip_prefix("file.open_recent:") {
                let _ = app.emit("menu-open-recent", path);
            }
        }
    }
}
