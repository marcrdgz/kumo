# AGENTS.md - Agent Instructions & Workspace Context

## Project Overview
**Neomux** is a high-performance terminal multiplexer built natively in **Zig** on top of Ghostty's core VT library (`libghostty`), featuring native Claude AI integration and TMUX-style session/pane management.

### Technology Stack
- **Primary Language**: Zig (`build.zig`)
- **Terminal & VT Library**: Ghostty core (`libghostty` / Ghostty internal modules)
- **AI Integration**: Claude API / CLI (HTTP/JSON streaming or IPC)
- **Target OS**: macOS (Darwin / AppKit / Metal), Linux (GTK / Wayland / X11)


---

## Language & Communication Standard
- **Language**: Write all technical code, architecture documentation, code comments, commit messages, and plans in **English**.
- **Domain Terminology**: Use precise technical language appropriate for terminal emulation and system programming:
  - **Terminal System**: PTY (*Pseudo-terminal*), TTY master/slave, VT100/Xterm emulation, ANSI escape parsing, scrollback buffer, raw mode, IPC, UNIX domain sockets.
  - **Multiplexer**: Leader key, split binary tree, horizontal/vertical panes, session detaching/reattaching, active grid buffer, status bar.
  - **AI Integration**: Context extraction, scrollback pipeline, command traceback analysis, execution stream.

---

## Codebase Principles & Rules
1. **Never Guess Logic or File Structures**: Inspect authoritative source files or `libghostty` headers before referencing C/Zig/Rust bindings.
2. **Clean Abstractions**: Keep VT emulation (`libghostty`), layout tree management, IPC server daemon, and Claude AI integration modular and loosely coupled.
3. **No Superficial Fixes**: Always verify PTY process lifecycle, file descriptor cleanup, and signal handling (`SIGWINCH`, `SIGCHLD`) thoroughly.
4. **Verification**: Always run build/test commands after editing code to verify compilation and execution success.

---

## Key Architecture References
- **`PLAN.md`**: Contains the full high-level implementation roadmap and component breakdowns.
