# AGENTS.md - Agent Instructions & Workspace Context

## Project Overview
**Kumo** is a terminal multiplexer with a Rust TUI frontend built with
**ratatui**, using **libghostty-vt** (Ghostty's headless terminal emulator,
vendored as C via Zig) for terminal emulation and **portable-pty** for PTY
management, featuring Claude AI integration and TMUX-style session/pane
management.

### Architecture
The daemon and the clients are **one binary plus a desktop app** in one cargo
workspace:

- **`app/kumo`** (`kumo`): the single binary — the headless **daemon** and the
  **TUI/control CLI** together. `kumo daemon` runs the headless server, `kumo`
  launches the TUI, `kumo session|pane|agent` drives it, `kumo update`
  self-updates. Sources are split into `src/daemon/` (PTYs, ghostty emulators,
  layout tree, agents) and `src/cli/` (chrome, input, mouse, control CLI).
- **`app/desktop`** (`kumo-desktop`): a native GPUI desktop client that draws
  its own chrome. Distributed separately (as a `Kumo-<arch>.dmg` on macOS);
  it is the "smart" update interface — on launch it checks the `kumo` CLI and
  the app itself against the latest release and can update both in-app.
- **`crates/kumo-core`**: shared logic — config, layout tree, themes, update
  check, the desktop updater (`updater.rs`), worktrees.
- **`crates/kumo-protocol`**: the pure wire protocol (`Command`/`DaemonEvent`).

The client and desktop launch the daemon via `kumo daemon` (sibling binary or
`PATH`); the daemon restarts itself in place for `kumo update`
(`kumo daemon --resume <file>`).

### Technology Stack
- **Frontend**: Rust TUI (ratatui + crossterm) in `app/kumo/src/cli`.
- **Terminal Emulator**: `libghostty-vt` vendored in `vendor/`, compiled at
  build time by `app/kumo/build.rs` via `zig build -Demit-lib-vt`, driven
  through a hand-written C FFI layer in `app/kumo/src/daemon/vt.rs`.
- **PTY Management**: `portable-pty` (`app/kumo/src/daemon/pty.rs`).
- **AI Integration**: Claude CLI/agent running in a dedicated AI pane, with
  live CPU/RAM sampling (`app/kumo/src/daemon/app/proc.rs`).
- **Updates**: `crates/kumo-core/src/update.rs` (the CLI self-updater) and
  `crates/kumo-core/src/updater.rs` (the desktop app's manager: checks the
  `kumo` CLI and the app against the latest release, installs the CLI via
  `install.sh`, self-updates the app via the `.dmg`).
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
2. **Clean Abstractions**: Keep PTY/process management (`app/kumo/src/daemon/pty.rs`), the terminal emulator FFI (`app/kumo/src/daemon/vt.rs`), layout tree management (`crates/kumo-core/src/layout.rs`), and Claude AI integration modular and loosely coupled.
3. **No Superficial Fixes**: Always verify PTY process lifecycle, file descriptor cleanup, and signal handling (`SIGWINCH`, `SIGCHLD`) thoroughly.
4. **Verification**: Always run build/test commands after editing code to verify compilation and execution success.
5. **Lint Clean**: Always run `cargo clippy --workspace` and fix all warnings before finishing a task — a task is not done while clippy is noisy.
6. **Never commit without asking**: Do not run `git commit` (or amend/push) unless the user explicitly asks. Stage nothing on your own; leave commits to the user.

---

## Build & Test Commands
- **Build**: `cargo build --workspace` (from the workspace root)
- **Run**: `cargo run -p kumo` (or `make run`) — the client; the daemon is the same
  binary: `target/debug/kumo daemon` (or `cargo run -p kumo -- daemon`)
- **Tests**: `cargo test --workspace`
- **Lint**: `cargo clippy --workspace` — must be warning-free before finishing a task
- **Note**: building `kumo` requires a `zig` toolchain on `PATH` to compile the
  vendored `libghostty-vt` (the `build.rs` lives in `app/kumo`). The vendored
  build pins zig `0.15.x` (it rejects `0.16+`). On macOS with Xcode ≥ 26.5,
  also point builds at the CommandLineTools SDK
  (`DEVELOPER_DIR=/Library/Developer/CommandLineTools`; the Makefile exports
  it): the Xcode 26.5 SDK's `libSystem.tbd` no longer lists the `arm64-macos`
  slice, so zig 0.15 fails every native link with undefined libSystem symbols.

## Commit Convention
- Use **Conventional Commits**: `<type>(<scope>): <summary>` where `type` is one of `feat`, `fix`, `refactor`, `chore`, `docs`, `test`, `build`.
- Keep the summary lowercase and imperative, under ~72 chars.
- Do not add a body/description; use only the header line.

---

## Key Architecture References
- **`app/kumo/src/main.rs`**: binary entry — dispatcher (`kumo daemon` | control CLI + update | TUI launch).
- **`app/kumo/src/daemon/app.rs`**: the engine — sessions, layout tree ops, PTYs, agents, themes.
- **`app/kumo/src/daemon/app/server.rs`**: headless daemon loop — command dispatch, per-client streams.
- **`app/kumo/src/daemon/app/commands.rs`**: the daemon's command handlers (sessions/panes/agents).
- **`app/kumo/src/daemon/pane.rs`**: Pane = PTY (`app/kumo/src/daemon/pty.rs`) + ghostty terminal.
- **`app/kumo/src/daemon/agents/`**: Per-agent lifecycle detection (`opencode.rs`, `claude.rs`),
  dispatched by `app/kumo/src/daemon/agents/mod.rs` from a `Snapshot` of the terminal buffer.
- **`app/kumo/src/daemon/vt.rs`**: Hand-written FFI bindings to the `libghostty-vt` C API
  plus the safe `Terminal` wrapper.
- **`app/kumo/src/cli/client_view.rs`**: client-side view — geometry, input, mouse, and all chrome rendering.
- **`app/kumo/build.rs`**: Compiles the vendored Zig library at build time.
- **`crates/kumo-core/src/updater.rs`**: the desktop app's update manager (CLI bootstrap + `.dmg` self-update).
- **`crates/kumo-core/src/daemon.rs`**: locating/spawning the `kumo` daemon binary (`kumo daemon`).
- **`crates/kumo-core/src/`**: shared config, layout, theme, update, worktrees.
- **`crates/kumo-protocol/`**: the wire protocol.
- **`vendor/libghostty-vt/`**: Vendored Ghostty terminal emulator (Zig + C headers).
- **`PLAN.md`**: Contains the full high-level implementation roadmap and component breakdowns.
- **`ROADMAP.md`**: User-facing roadmap of planned features (theme, keymaps, config, chrome, plugins).
