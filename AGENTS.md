# AGENTS.md - Agent Instructions & Workspace Context

## Project Overview
**Kumo** is a terminal multiplexer with a single Rust TUI frontend built with
**ratatui**, using **libghostty-vt** (Ghostty's headless terminal emulator,
vendored as C via Zig) for terminal emulation and **portable-pty** for PTY
management, featuring Claude AI integration and TMUX-style session/pane
management.

### Technology Stack
- **Frontend**: Rust TUI (ratatui + crossterm), `src/` crate (`kumo-tui`).
- **Terminal Emulator**: `libghostty-vt` vendored in `vendor/`, compiled at
  build time by `src/build.rs` via `zig build -Demit-lib-vt`, driven through
  a hand-written C FFI layer in `src/src/vt.rs`.
- **PTY Management**: `portable-pty` (shared `kumo-core` crate).
- **AI Integration**: Claude CLI/agent running in a dedicated AI pane.
- **Target OS**: macOS (primary dev), Linux, Windows.

---

## Language & Communication Standard
- **Language**: Write all technical code, architecture documentation, code comments, commit messages, and plans in **English**.
- **Domain Terminology**: Use precise technical language appropriate for terminal emulation and system programming:
  - **Terminal System**: PTY (*Pseudo-terminal*), TTY master/slave, VT100/Xterm emulation, ANSI escape parsing, scrollback buffer, raw mode, IPC, UNIX domain sockets.
  - **Multiplexer**: Leader key, split binary tree, horizontal/vertical panes, session detaching/reattaching, active grid buffer, status bar.
  - **AI Integration**: Context extraction, scrollback pipeline, command traceback analysis, execution stream.

---

## Codebase Principles & Rules
1. **Never Guess Logic or File Structures**: Inspect authoritative source files before referencing Rust/C bindings.
2. **Clean Abstractions**: Keep PTY/process management (`kumo-core`), the terminal emulator FFI (`vt.rs`), layout tree management, and Claude AI integration modular and loosely coupled.
3. **No Superficial Fixes**: Always verify PTY process lifecycle, file descriptor cleanup, and signal handling (`SIGWINCH`, `SIGCHLD`) thoroughly.
4. **Verification**: Always run build/test commands after editing code to verify compilation and execution success.

---

## Build & Test Commands
- **Build**: `cargo build` (from the workspace root)
- **Run**: `cargo run -p kumo-tui` (or `make run`)
- **Tests**: `cargo test`
- **Lint**: `cargo clippy`
- **Note**: building requires a `zig` toolchain on `PATH` to compile the
  vendored `libghostty-vt`.

## Commit Convention
- Use **Conventional Commits**: `<type>(<scope>): <summary>` where `type` is one of `feat`, `fix`, `refactor`, `chore`, `docs`, `test`, `build`.
- Keep the summary lowercase and imperative, under ~72 chars.
- Do not include a body unless it adds meaningful context.

---

## Key Architecture References
- **`src/src/main.rs`**: TUI entry point.
- **`src/src/app.rs`**: Session/pane tree, input routing, mouse, rendering.
- **`src/src/pane.rs`**: Pane = PTY (`kumo-core`) + ghostty terminal.
- **`src/src/vt.rs`**: Hand-written FFI bindings to the `libghostty-vt` C API
  plus the safe `Terminal` wrapper.
- **`src/build.rs`**: Compiles the vendored Zig library at build time.
- **`kumo-core/`**: Shared PTY and config layer.
- **`vendor/libghostty-vt/`**: Vendored Ghostty terminal emulator (Zig + C headers).
- **`PLAN.md`**: Contains the full high-level implementation roadmap and component breakdowns.
