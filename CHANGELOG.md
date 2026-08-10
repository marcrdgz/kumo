# Changelog

## [0.2.0] - 2026-08-10

### 🚀 Features

- *(agents)* Split per-agent detection and add claude support
- *(ui)* Add split and close actions to context menu
- *(ui)* Add leader+? keybind showcase
- *(cli)* Add -h/--help usage output

### 🐛 Bug Fixes

- *(mouse)* Drop trailing sgr reset from forwarded mouse events
- *(ui)* Rename detach binding to exit

### 🚜 Refactor

- *(update)* Replace gh with direct https

### 📚 Documentation

- Tighten commit convention and list roadmap

### ⚙️ Miscellaneous Tasks

- Generate changelog with git-cliff on release
## [0.1.0] - 2026-08-10

### 🚀 Features

- Native app engine
- Add AI CLI pane via leader key
- Pane close button, context-to-AI, ctrl-space leader
- Send vim visual selection as AI context
- Persist and restore session layout
- Dynamic pane titles via process tree polling
- Native copy/paste and scrollback search
- Workspace management, recent folders, and native macOS menu
- Multi-session tabs and git panel
- Session sidebar with git panel and per-session worktrees
- Ratatui new ui
- Migrate TUI to libghostty-vt and make kumo TUI-only
- Session/agent sidebar, opencode states, and mac word-delete
- Detect AI CLI panes and present a plain-tty identity
- Print version with -v/--version
- Detect more AI CLIs and add DEBUG_AGENT-gated status log
- Use libghostty-vt native selection for drag-select
- Open in the directory kumo was launched from
- Detect agent state from terminal screen markers
- Agent sidebar shows workspace and CLI name with focus highlight
- Show leader keys in status bar and fix pane close crash
- .config/kumo config
- Status-bar menu with config/detach dropdown and mouse hover
- Scrollable sidebar sessions and agents sections with scrollbars
- Session-name popup with editable field and enter/esc buttons
- Add pane context menu with rename
- Rename sessions via right-click context menu
- Jump to session by number with leader+1-9
- Add self-update and cargo-dist release pipeline
- Alert with sound when agents block or finish

### 🐛 Bug Fixes

- Cmd+backspace deletes whole line in panes
- Avoid null-vtable fat pointer that crashed release builds
- Detect blocked AI state through ANSI and fix scroll direction
- Keep cached pane rows across partial dirty renders
- Extract selection from a fresh viewport range like a terminal
- Forward mouse gestures to mouse-reporting panes
- Detect agent state from buffer tail, not viewport
- Default shell fallback to bash
- Clicking first sidebar session switches sessions
- Split new pane to the right/below and keep focus
- Number shell panes sequentially per session
- Dedupe in-flight nightly builds with a claim marker
- Clean up nightly tag on release reset
- Discard gh api stdout when reading nightly claim ref
- Send force as raw boolean when updating nightly claim ref
- Upload flattened nightly assets instead of artifacts dir

### 💼 Other

- Native terminal backgrounds for panes and chrome
- Render update banner on two lines with separated close button

### 🚜 Refactor

- Migrate from zig to tauri v2 stack
- Merge kumo-core and move manifest to workspace root
- Split app.rs into app/mouse, ui, sidebar, overlays, tasks, util

### 📚 Documentation

- Add README
- Add rule to never commit without asking
- Add customization roadmap to README
- Reorder README sections and note negligible resource usage

### ⚡ Performance

- Render dirty rows via render-state patch and cache pane buffers

### 🎨 Styling

- Center kumo header in sidebar

### ⚙️ Miscellaneous Tasks

- Bump workspace to 0.0.0-dev
- Sync Cargo.lock to 0.0.0-dev
- Add license field to Cargo.toml
- Move cargo-release config to workspace.metadata.release
- Drop unused .cargo/release.toml
- Build nightly on 2 targets and cache zig builds
- Release 0.1.0
