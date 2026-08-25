# Changelog

## v0.7.0-prerelease.1

### 🚀 Features

- *(cli)* Add per-subcommand help and keep -h/-v top-level only
- *(agents)* Five-state agent model (working · blocked · idle · done · unknown) (#4)
- *(ui)* Agent inbox with divided sidebar panels (#5)

### 🐛 Bug Fixes

- *(ci)* Allow cargo-dist dirty check for hand-edited release.yml

### 💼 Other

- *(ui)* Remove 1-cell separator between panes
- *(roadmap)* Update 0.7.0

### ⚙️ Miscellaneous Tasks

- *(ci)* Restrict release workflow to tags on main
## v0.6.0

### 🚀 Features

- *(tabs)* Add per-session tabs with hover-close top bar
- *(tabs)* Polish tab bar visuals and interactions
- *(tabs)* Add plus button, overflow arrows, context menu and hover-targeted close/rename
- *(theme)* Add owned theme, hex parsing and gruvbox/dracula/tokyo-night
- *(protocol)* Add wire theme and bump protocol to v7
- *(daemon)* Handle dynamic and reloadable themes
- *(cli)* Support custom theme in picker and rendering
- *(sidebar)* Toggle/order sections, rounded borders default with hidden
- *(core)* Add status bar config schema
- *(cli)* Add status bar widget module
- *(cli)* Integrate status bar widgets into client view
- *(copy-mode)* Vi-style selection over scrollback plus search
- *(copy-mode)* Search as black/red context menu rectangle below tabs
- *(docs)* Static doc for kumo (#2)
- *(daemon)* Desktop notifications for agent lifecycle transitions
- *(config)* Require manual opt-in for agent notifications
- *(ui)* Add toast notifications
- *(notifications)* Add position and sound knobs to [notifications]
- *(docs)* Add interactive config builder to the configuration page
- *(daemon)* Re-check for updates periodically to notify long-running daemons

### 🐛 Bug Fixes

- *(tabs)* Use text color for active tab close x
- *(tabs)* Left-align tab label in first cell
- *(tabs)* Keep absolute numbering after rename and prefer name over id
- *(copy-mode)* Add leader+/ to enter search and clarify flow
- *(copy-mode)* Use readable black on input_bg for search bar
- *(copy-mode)* Move search/hint to popup two lines below tabs with gap
- *(copy-mode)* Show copy hint as status bar notification on right

### 💼 Other

- *(kumo)* Add chrono and hostname dependencies
- *(cli)* UI improvements (#3)

### ⚙️ Miscellaneous Tasks

- *(tabs)* Remove dead-code warnings
- *(workspace)* Remove kumo-desktop and fix clippy warnings
- Release 0.6.0
## v0.5.4

### 🐛 Bug Fixes

- *(pty)* Close inherited fd on drop
- *(daemon)* Forget proc sampler by os pid
- *(server)* Prune pane caches and fail closed peer check
- *(protocol)* Disconnect on oversized frame
- *(pty)* Make kill non-blocking and fix try_wait
- *(server)* Handle sigterm, reduce poll and bound threads
- *(cli)* Validate workspace and reject unknown commands
- *(core)* Reduce update timeout to 10s
- *(core)* Cache config with mtime to avoid per-access reload
- *(agents)* Avoid per-pane to_lowercase alloc with case-insensitive search
- *(server)* Cache layout with version and arc to avoid clones
- *(vt)* Reuse scratch buffer to avoid per-cell string alloc
- *(daemon)* Preserve scrollback across restart via inline snapshot
- *(client)* Restore selection highlight and scope to correct pane
- *(pty)* Gate SIGKILL fallback behind cfg(unix) for Windows builds
- *(pty)* Implement as_raw_handle for DummyChild on Windows

### ⚙️ Miscellaneous Tasks

- Release 0.5.4
## v0.5.3

### 🐛 Bug Fixes

- *(claude)* Track cursor movement
- *(ui)* Pin wide emoji width and handle daemon restarting
- *(ui)* Use emulator grapheme width instead of hardcoded emoji table

### ⚙️ Miscellaneous Tasks

- Release 0.5.3
## v0.5.2

### 🚀 Features

- Update libghostty-vt to latest upstream (zig 0.16, new APIs)
- Implement bell callback to trigger audible alerts on BEL character
- Implement OSC 52 clipboard write callback for system clipboard integration

### 🐛 Bug Fixes

- Enable grapheme cluster mode (2027) for multi-codepoint emoji rendering
- *(client)* Avoid clone in render_pane_content borrow
- *(client)* Highlight only focused agent in active session
- *(agents)* Filter dingbat completion summaries as working
- *(ui)* Remove duplicate status dot in agents tab
- *(ci)* Bump zig to 0.16.0 for libghostty-vt compatibility
- *(ci)* Bump zig to 0.16.0 in dist-build-setup for libghostty-vt

### ⚡ Performance

- Reduce render loop latency (daemon 8ms→4ms, client 16ms→8ms)
- Optimize dirty tracking (early-exit + selective clear)
- Use batch queries in refresh() to reduce FFI overhead
- Use batch queries in for_each_cell to reduce FFI overhead
- *(client)* Cache rendered rows to avoid O(cols×rows) per frame
- *(daemon)* Move git status and ps to background threads
- *(daemon)* Wrap client socket in BufWriter to reduce syscalls
- *(daemon)* Eliminate buf.clone() with persistent pane buffer cache

### ⚙️ Miscellaneous Tasks

- Release 0.5.2
## v0.5.1

### 🐛 Bug Fixes

- *(release)* Regenerate the root changelog from the workspace root
- Input lag

### 💼 Other

- *(release)* Scope cargo-release to the kumo package

### ⚙️ Miscellaneous Tasks

- Ignore the zcode agent session directory
- Release 0.5.1
## v0.5.0

### 🚀 Features

- *(theme)* Add theme picker with settings popup
- *(settings)* Redesign theme picker as a tabbed settings panel
- *(sidebar)* Tabbed sessions/agents panel
- *(sessions)* Create and open git worktrees
- *(ui)* Widen pane left gutter and slim sidebar by one column
- *(links)* Open URLs on modifier+click and underline while held
- *(mouse)* Keep selection highlighted and toast on copy
- *(popup)* Word-delete editing in the rename/new popups
- *(status)* Show copy confirmation right-aligned in the status bar

### 🐛 Bug Fixes

- *(mouse)* Restore mouse tracking on resumed panes
- *(theme)* Use secondary accent instead of hardcoded mauve
- *(sidebar)* Extend active row highlight to full width
- *(sidebar)* Make git branch rows clickable
- *(restart)* Repaint resumed panes in inactive sessions
- *(build)* Gate the control cli module behind unix

### 💼 Other

- Daemon + smart-client architecture, workspace split (cli/daemon/desktop/kumo-core) (#1)
- *(release)* Stop shipping the desktop app until it's useful

### ⚙️ Miscellaneous Tasks

- Ignore cargo-release per-crate changelogs
- Release 0.5.0
## v0.4.0

### 🚀 Features

- *(ui)* Highlight active session row in sidebar
- *(ui)* Show git ahead/behind on sidebar branch
- *(config)* Migrate config to TOML
- *(config)* Make the leader chord configurable
- *(keymap)* Make leader bindings configurable
- *(keymap)* Add resize, swap/rotate and pane numbers
- *(config)* [terminal] new-cwd policy with follow-workspace
- *(reload)* Kumo reload applies config live
- *(ui)* MENU config opens editor, adds reload item
- *(cli)* Kumo server restart restarts the daemon in place
- *(paste)* Strip trailing newline on paste

### 🐛 Bug Fixes

- *(ui)* Replace chrome yellow with mauve and wire named colors
- *(follow)* Prefer process-tree cwd over OSC 7
- *(follow)* Track the foreground process group for cwd
- *(ci)* Move dist build-setup outside workflows dir
- *(agents)* Detect claude working from prompt-box markers
- *(daemon)* Recover lagging clients instead of freezing them

### 🚜 Refactor

- *(bindings)* Drive dispatch from the shared keymap table

### ⚙️ Miscellaneous Tasks

- Release 0.4.0
## v0.3.0

### 🚀 Features

- *(ui)* Add zoom/unzoom to pane context menu
- *(state)* Detach saves state and re-attach restores sessions
- *(ui)* Rename exit to detach in leader key and menu
- *(daemon)* Headless server renders frames for thin clients
- *(daemon)* Dirty-row diffs and kumo ls/kill control CLI
- *(daemon)* Kumo new creates a session in a running daemon
- *(daemon)* Reject socket connections from other users
- *(daemon)* Surface agent status in kumo ls
- *(daemon)* Restart in place so kumo update keeps panes alive

### 🐛 Bug Fixes

- *(render)* Skip wide-char continuations so scrolls leave no ghost cells
- *(render)* Keep full emoji graphemes through the daemon pipeline
- *(render)* Stop trailing erase from deleting the right pane border
- *(daemon)* Build server module on unix only
- *(release)* Gate resume path to unix so 0.3.0 builds on windows

### 💼 Other

- Prefix changelog headings with v for release titles
- Exclude docs commits from changelog
- Add libc and bincode for the daemon client-server

### 🎨 Styling

- Navy menu backgrounds and normal blue accent

### ⚙️ Miscellaneous Tasks

- Release 0.3.0
## v0.2.1

### 🐛 Bug Fixes

- *(update)* Pick platform-appropriate cargo-dist installer
- *(app)* Open context menu down-right, flipping up/left when needed

### ⚙️ Miscellaneous Tasks

- Release 0.2.1
## v0.2.0

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

### ⚙️ Miscellaneous Tasks

- Generate changelog with git-cliff on release
- Release 0.2.0
## v0.1.0

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
