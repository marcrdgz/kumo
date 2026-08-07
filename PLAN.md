# Neomux: Tauri-based Terminal Multiplexer with Claude AI Integration

## 1. Executive Summary
**Neomux** is a terminal multiplexer built with **Tauri v2** (Rust backend + TypeScript frontend) using **xterm.js** for terminal rendering, with deep native integration for AI assistance via Claude.

---

## 2. Core Architecture

### A. Terminal Emulation & Rendering (xterm.js in webview)
- Rust backend manages PTYs via `portable-pty` and streams output to the frontend.
- Frontend renders VT100/Xterm output with `@xterm/xterm` (canvas/WebGL renderer) inside a Tauri webview.
- Output flows Rust → `pane-output` Tauri events → xterm.js; input flows `onData` → `write_pane` → PTY.

### B. Layout & Pane Manager
- **Hierarchical Layout Tree**: Manage panes using a split tree structure (horizontal & vertical splits) in the frontend.
- **Pane Management**: Focus switching, pane resizing, and closing (binary tree of splits, DOM-based layout).
- **Session Persistence & Daemon**: Session state (name, panes, active pane) lives in the Rust backend `AppState`; panes emit `pane-closed` on exit.

### C. Claude AI Deep Integration
- **Specialized AI Pane**: Dedicated UI split running an interactive Claude stream/agent.
- **Context Sharing**: Pipeline output, scrollback buffer, or recent command errors directly to Claude's prompt context.
- **Action Execution**: Allow Claude to suggest or execute verified commands directly inside target terminal panes.

---

## 3. High-Level Implementation Roadmap

1. **Phase 1: PTY & Shell**
   - Rust backend spawning PTYs via `portable-pty` with raw input/output passthrough.
   - Frontend xterm.js terminal wired to `pane-output` events and `write_pane` input.

2. **Phase 2: Split Layout Engine & Keybindings**
   - Implement split-pane layout tree (binary tree of splits) in the frontend.
   - Implement keybinding dispatch system (e.g., Leader key `Ctrl+A` or `Ctrl+B`).

3. **Phase 3: Session Management**
   - Extend Rust `AppState` with multi-session support, listing and switching between sessions.

4. **Phase 4: Claude AI Agent Integration**
   - Build API client integration for Claude API / CLI.
   - Implement smart context extractions (e.g., send pane buffer, analyze error tracebacks).

5. **Phase 5: Polish & UI Customization**
   - Theme configuration, status line customization, modal menus, and keyboard shortcuts.

---

## 4. Agent Guidelines & English Technical Terminology

To ensure high-quality English technical output during development:
- **Terminal Concepts**: PTY (Pseudo-Terminal), TTY, Scrollback Buffer, VT100/Xterm Emulation, ANSI Escape Sequences, Raw Mode, Non-canonical Mode, IPC (Inter-Process Communication), Tauri events/invoke.
- **Multiplexer Terminology**: Leader Key, Split Tree, Panes, Windows, Sessions, Detach/Reattach, Grid Buffer.
- **Codebase Style**: Use clear, concise commit messages, complete docstrings, precise technical phrasing, and modular file organization.

---

## 5. Key Source Files
- **`src-tauri/src/commands.rs`**: Tauri command handlers (create/split/attach/write/resize/close, shell resolution).
- **`src-tauri/src/session.rs`**: `AppState` (session/PTY registry, pane tree bookkeeping) and IPC request structs.
- **`src-tauri/src/pty.rs`**: `portable-pty` wrapper (spawn, read loop, resize, kill).
- **`src/multiplexer.ts`**: Frontend layout tree, split/close logic, session init.
- **`src/terminal.ts`**: xterm.js wrapper (fit, data routing, base64 decode).
- **`src/api.ts`**: Typed IPC bindings for Tauri events and invoke commands.
