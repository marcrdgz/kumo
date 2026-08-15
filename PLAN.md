# Kumo: Rust TUI Terminal Multiplexer with Claude AI Integration

## 1. Executive Summary
**Kumo** is a terminal multiplexer with a single **Rust TUI** frontend built on
**ratatui**, using **libghostty-vt** (Ghostty's headless terminal emulator) for
terminal emulation and **portable-pty** for PTY management, with deep
integration for AI assistance via Claude.

---

## 2. Core Architecture

### A. Terminal Emulation & Rendering (libghostty-vt + ratatui)
- Each pane owns a PTY (`portable-pty`) plus a `libghostty-vt`
  terminal instance (see `src/vt.rs` FFI).
- PTY output is fed into the emulator via `ghostty_terminal_vt_write`.
- Every frame the emulator's render state is refreshed and its viewport cells
  (text, colors, styles, graphemes) are drawn into a ratatui `Buffer`.
- Input flows `crossterm` events → key encoding → PTY.

### B. Layout & Pane Manager
- **Hierarchical Layout Tree**: binary split tree (horizontal & vertical).
- **Pane Management**: focus switching, mouse-drag resize, closing, zoom.
- **Sessions**: independent layout trees, tab bar, sidebar listing panes.

### C. Claude AI Deep Integration
- **Specialized AI Pane**: dedicated split running an interactive Claude
  agent/CLI.
- **Context Sharing**: pipeline output, scrollback buffer, or recent command
  errors directly to Claude's prompt context.

---

## 3. High-Level Implementation Roadmap

1. **Phase 1: PTY & Shell**
   - Spawn PTYs via `portable-pty` with raw input/output passthrough.
   - Wire the `libghostty-vt` emulator to render shell output into ratatui.
   - Answer terminal query responses (DA/DSR/OSC) so shells don't block.

2. **Phase 2: Split Layout Engine & Keybindings**
   - Split-pane binary tree with resizing (mouse + keys).
   - Leader key (`Ctrl+B`) dispatch system.

3. **Phase 3: Session Management**
   - Multiple sessions with independent layout trees; switch/cycle.

4. **Phase 4: Claude AI Agent Integration**
   - Dedicated AI pane (CLI), context extraction, command execution.

5. **Phase 5: Polish & UI Customization**
   - Theme configuration, status line customization, modal menus, shortcuts.

---

## 4. Agent Guidelines & English Technical Terminology

To ensure high-quality English technical output during development:
- **Terminal Concepts**: PTY (Pseudo-Terminal), TTY, Scrollback Buffer, VT100/Xterm Emulation, ANSI Escape Sequences, Raw Mode, Non-canonical Mode, IPC, Render State.
- **Multiplexer Terminology**: Leader Key, Split Tree, Panes, Windows, Sessions, Detach/Reattach, Grid Buffer.
- **Codebase Style**: Use clear, concise commit messages, complete docstrings, precise technical phrasing, and modular file organization.

---

## 5. Architecture: smart renderer / dumb viewport (protocol v4)

The daemon is the **single source of truth** for everything it has open —
sessions, the **semantic layout tree** (splits in ratios, never pixels), the
PTYs, and per-pane terminal content — and it **never renders chrome**: no
borders, box-drawing characters, sidebar, or status bar ever enter the wire.
Clients are **dumb viewports**: they receive two things and draw everything
themselves (through `crates/kumo-protocol`):

- **Layout** (`DaemonEvent::Layout`): sessions → splits (with ratios) → panes
  (title, cwd, agent status). Clients compute geometry, request pane sizes via
  `PaneResize`, and draw their own borders/chrome.
- **Pane content** (`DaemonEvent::PaneFrame`): each pane's terminal grid
  (rendered by the daemon's Ghostty core), streamed on change.

Everything else is a **command** (`Command`), tmux/zellij style — the whole
multiplexer is drivable from the CLI, the TUI, the desktop app, or a script:

- `kumo session [list|new|kill|attach]`
- `kumo pane [split|close|focus|send-keys]`
- `kumo agent [spawn|status|kill]`

The daemon's keyboard layer is gone: the TUI client owns the leader keymap
(`src/app/bindings.rs` is shared) and translates keys into commands
(`src/client.rs`). Opening/closing a sidebar or resizing is a client concern
that never mutates the daemon's state beyond a `PaneResize`/command.

## 6. Key Source Files
- **`src/app/server.rs`**: headless daemon loop — command dispatch, per-client
  routing, layout + pane-frame streaming.
- **`src/app/commands.rs`**: the daemon's command handlers (sessions/panes/agents).
- **`src/app/ui.rs`**: per-pane content rendering (`tick`) and the semantic
  `layout()` export; no chrome.
- **`src/client.rs`**: the TUI client — a dumb viewport that lays out from the
  semantic tree, draws borders, and maps the leader keymap to commands.
- **`src/cli.rs`**: the `kumo session|pane|agent` control CLI.
- **`crates/kumo-protocol/`**: `Command`/`DaemonEvent`, the semantic
  `LayoutNode`/`Layout`, `PaneFrame`, and pure framing.
- **`src/frames.rs`**: daemon-side per-pane `Buffer` → `PaneFrame` serialization.
- **`src/app.rs`**: the engine — sessions, layout tree ops, PTYs, agents.
- **`apps/kumo-desktop/`**: native macOS desktop client (GPUI) — computes its
  own geometry from the semantic tree and paints native pane cards.
- **`src/pane.rs`**: `Pane` = PTY + `libghostty-vt` terminal.
- **`build.rs`**: compiles the vendored `libghostty-vt` Zig library.
- **`src/config.rs`**: XDG directory resolution, config parsing.
- **`vendor/libghostty-vt/`**: vendored Ghostty terminal emulator (Zig + C).
