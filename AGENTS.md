# AGENTS.md - Agent Instructions & Workspace Context

## Project Overview
**Kumo** is a terminal multiplexer with a Rust TUI frontend built with
**ratatui**, using **libghostty-vt** (Ghostty's headless terminal emulator,
vendored as C via Zig) for terminal emulation and **portable-pty** for PTY
management, featuring Claude AI integration and TMUX-style session/pane
management.

### Architecture
The daemon and the clients are **separate binaries** in one cargo workspace:

- **`app/daemon`** (`kumo-daemon`): the headless daemon — owns the PTYs, the
  ghostty terminal emulators, the semantic layout tree, and agent metadata,
  served over a unix socket. It **never renders chrome**.
- **`app/cli`** (`kumo`): the client — the TUI (draws ALL chrome: borders,
  sidebar, status bar, menus, popups) plus the `kumo session|pane|agent`
  control CLI and `kumo update`. It has no terminal emulator and no `zig`
  build step.
- **`app/desktop`** (`kumo-desktop`): a native GPUI desktop client that also
  draws its own chrome.
- **`crates/kumo-core`**: shared logic — config, layout tree, themes, update
  check, worktrees.
- **`crates/kumo-protocol`**: the pure wire protocol (`Command`/`DaemonEvent`).

The client and desktop launch `kumo-daemon` (sibling binary or `PATH`); the
daemon restarts itself in place for `kumo update` (`--resume <file>`).

### Technology Stack
- **Frontend**: Rust TUI (ratatui + crossterm) in `app/cli`.
- **Terminal Emulator**: `libghostty-vt` vendored in `vendor/`, compiled at
  build time by `app/daemon/build.rs` via `zig build -Demit-lib-vt`, driven
  through a hand-written C FFI layer in `app/daemon/src/vt.rs`.
- **PTY Management**: `portable-pty` (`app/daemon/src/pty.rs`).
- **AI Integration**: Claude CLI/agent running in a dedicated AI pane, with
  live CPU/RAM sampling (`app/daemon/src/app/proc.rs`).
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
2. **Clean Abstractions**: Keep PTY/process management (`app/daemon/src/pty.rs`), the terminal emulator FFI (`app/daemon/src/vt.rs`), layout tree management (`crates/kumo-core/src/layout.rs`), and Claude AI integration modular and loosely coupled.
3. **No Superficial Fixes**: Always verify PTY process lifecycle, file descriptor cleanup, and signal handling (`SIGWINCH`, `SIGCHLD`) thoroughly.
4. **Verification**: Always run build/test commands after editing code to verify compilation and execution success.
5. **Never commit without asking**: Do not run `git commit` (or amend/push) unless the user explicitly asks. Stage nothing on your own; leave commits to the user.

---

## Build & Test Commands
- **Build**: `cargo build --workspace` (from the workspace root)
- **Run**: `cargo run -p kumo` (or `make run`) — the client; `cargo run -p kumo-daemon` — the daemon
- **Tests**: `cargo test --workspace`
- **Lint**: `cargo clippy --workspace`
- **Note**: building `kumo-daemon` (and `kumo-desktop`'s daemon dependency) requires a `zig`
  toolchain on `PATH` to compile the vendored `libghostty-vt`; the `kumo` client builds without it.

## Commit Convention
- Use **Conventional Commits**: `<type>(<scope>): <summary>` where `type` is one of `feat`, `fix`, `refactor`, `chore`, `docs`, `test`, `build`.
- Keep the summary lowercase and imperative, under ~72 chars.
- Do not add a body/description; use only the header line.

---

## Key Architecture References
- **`app/cli/src/main.rs`**: client entry (control CLI + update + TUI launch).
- **`app/cli/src/client_view.rs`**: client-side view — geometry, input, mouse, and all chrome rendering.
- **`app/daemon/src/app.rs`**: the engine — sessions, layout tree ops, PTYs, agents, themes.
- **`app/daemon/src/app/server.rs`**: headless daemon loop — command dispatch, per-client streams.
- **`app/daemon/src/app/commands.rs`**: the daemon's command handlers (sessions/panes/agents).
- **`app/daemon/src/pane.rs`**: Pane = PTY (`app/daemon/src/pty.rs`) + ghostty terminal.
- **`app/daemon/src/agents/`**: Per-agent lifecycle detection (`opencode.rs`, `claude.rs`),
  dispatched by `app/daemon/src/agents/mod.rs` from a `Snapshot` of the terminal buffer.
- **`app/daemon/src/vt.rs`**: Hand-written FFI bindings to the `libghostty-vt` C API
  plus the safe `Terminal` wrapper.
- **`app/daemon/build.rs`**: Compiles the vendored Zig library at build time.
- **`crates/kumo-core/src/`**: shared config, layout, theme, update, worktrees.
- **`crates/kumo-protocol/`**: the wire protocol.
- **`vendor/libghostty-vt/`**: Vendored Ghostty terminal emulator (Zig + C headers).
- **`PLAN.md`**: Contains the full high-level implementation roadmap and component breakdowns.
- **`ROADMAP.md`**: User-facing roadmap of planned features (theme, keymaps, config, chrome, plugins).
