# AGENTS.md - Agent Instructions & Workspace Context

## Project Overview
**Neomux** is a terminal multiplexer built with **Tauri v2** (Rust backend + TypeScript frontend) using **xterm.js** for terminal rendering, featuring native Claude AI integration and TMUX-style session/pane management.

### Technology Stack
- **Backend**: Rust (Tauri v2 commands, `portable-pty` for PTY management)
- **Frontend**: TypeScript + Vite + xterm.js (`@xterm/xterm`, `@xterm/addon-fit`)
- **IPC**: Tauri events (`pane-output`, `pane-closed`) and invoke commands
- **AI Integration**: Claude API / CLI (HTTP/JSON streaming or IPC)
- **Target OS**: macOS (primary dev), Linux (via Tauri/webview support)


---

## Language & Communication Standard
- **Language**: Write all technical code, architecture documentation, code comments, commit messages, and plans in **English**.
- **Domain Terminology**: Use precise technical language appropriate for terminal emulation and system programming:
  - **Terminal System**: PTY (*Pseudo-terminal*), TTY master/slave, VT100/Xterm emulation, ANSI escape parsing, scrollback buffer, raw mode, IPC, UNIX domain sockets.
  - **Multiplexer**: Leader key, split binary tree, horizontal/vertical panes, session detaching/reattaching, active grid buffer, status bar.
  - **AI Integration**: Context extraction, scrollback pipeline, command traceback analysis, execution stream.

---

## Codebase Principles & Rules
1. **Never Guess Logic or File Structures**: Inspect authoritative source files before referencing Rust/C/TS bindings.
2. **Clean Abstractions**: Keep PTY/process management (Rust backend), layout tree management (frontend), IPC, and Claude AI integration modular and loosely coupled.
3. **No Superficial Fixes**: Always verify PTY process lifecycle, file descriptor cleanup, and signal handling (`SIGWINCH`, `SIGCHLD`) thoroughly.
4. **Verification**: Always run build/test commands after editing code to verify compilation and execution success.

---

## Build & Test Commands
- **Dev (hot reload)**: `npm run tauri dev`
- **Frontend build**: `npm run build`
- **Backend build**: `cargo build` (from `src-tauri/`)
- **Backend tests**: `cargo test` (from `src-tauri/`)

## Commit Convention
- Use **Conventional Commits**: `<type>(<scope>): <summary>` where `type` is one of `feat`, `fix`, `refactor`, `chore`, `docs`, `test`, `build`.
- Keep the summary lowercase and imperative, under ~72 chars.
- Do not include a body unless it adds meaningful context.

---

## Key Architecture References
- **`src-tauri/src/`**: Rust backend (`commands.rs`, `session.rs`, `pty.rs`).
- **`src/`**: TypeScript frontend (`multiplexer.ts` tree/layout, `terminal.ts` xterm wrapper, `api.ts` IPC bindings).
- **`PLAN.md`**: Contains the full high-level implementation roadmap and component breakdowns.
