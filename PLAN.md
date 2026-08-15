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

## 5. Client/Server Delivery Channels (protocol v2)

The daemon is the **single source of truth** for everything it has open
(sessions, panes, PTYs, agents) and serves clients with different capabilities
through the shared wire protocol (`crates/kumo-protocol`). The `Hello`
handshake carries a `ClientKind` so the daemon routes the right channels:

- **Full attach** (`Hello` + `Frame`): the daemon renders its whole UI
  headlessly and streams dirty-row `WireCell` patches. Used by the TUI client
  (`src/client.rs`) and the desktop app's main view.
- **Snapshot** (`SubscribeSnapshot` + `Snapshot`): structured
  `SessionInfo`/`PaneInfo`/`AgentInfo`, pushed on change. Drives native
  sidebars, session lists, and (future) mobile overviews.
- **Pane frames** (`SubscribePane` + `PaneFrame`): one pane rendered as its own
  grid, built from the retained `pane_cache`. Intended for per-pane views
  (mobile) and native pane layout (desktop) later.
- **Control** (`FocusSession`, `NewSession`, `Resize`, input/paste/mouse): any
  client can drive the same keymap/actions the TUI exposes.

The desktop app (`apps/kumo-desktop`, GPUI) is another client: it attaches with
`ClientKind::Desktop`, renders the composed frames in a native grid with full
keyboard/mouse input, and subscribes to snapshots for a native
sessions/agents sidebar. Several clients can be attached at once — terminal,
app, or both.

## 6. Key Source Files
- **`src/main.rs`**: TUI entry point.
- **`src/app.rs`**: sessions/panes, layout tree, input routing, mouse, rendering.
- **`src/pane.rs`**: `Pane` = PTY + `libghostty-vt` terminal.
- **`src/vt.rs`**: hand-written FFI bindings to `libghostty-vt` and the safe `Terminal` wrapper (write/resize/scroll/render/modes + query effects).
- **`src/protocol.rs`**: re-exports the shared wire protocol.
- **`src/frames.rs`**: daemon-side `ratatui` buffer → `FrameMsg`/`PaneFrame` serialization.
- **`src/app/server.rs`**: headless daemon loop, per-client routing, snapshot/pane-frame push.
- **`crates/kumo-protocol/`**: pure wire types + framing (no `ratatui`/`crossterm`; conversions gated behind the `crossterm` feature).
- **`apps/kumo-desktop/`**: native macOS desktop client (GPUI) — full-attach grid viewer + sessions/agents sidebar.
- **`build.rs`**: compiles the vendored `libghostty-vt` Zig library.
- **`src/pty.rs`**: `portable-pty` wrapper (spawn, read loop, resize, kill).
- **`src/config.rs`**: XDG directory resolution, Ghostty-style `~/.config/kumo/config` parser, shell/AI command resolution.
- **`vendor/libghostty-vt/`**: vendored Ghostty terminal emulator (Zig source + C headers).
