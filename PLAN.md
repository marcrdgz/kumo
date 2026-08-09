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

## 5. Key Source Files
- **`src/main.rs`**: TUI entry point.
- **`src/app.rs`**: sessions/panes, layout tree, input routing, mouse, rendering.
- **`src/pane.rs`**: `Pane` = PTY + `libghostty-vt` terminal.
- **`src/vt.rs`**: hand-written FFI bindings to `libghostty-vt` and the safe `Terminal` wrapper (write/resize/scroll/render/modes + query effects).
- **`build.rs`**: compiles the vendored `libghostty-vt` Zig library.
- **`src/pty.rs`**: `portable-pty` wrapper (spawn, read loop, resize, kill).
- **`src/config.rs`**: shell/AI command resolution and `~/.kumo` config.
- **`vendor/libghostty-vt/`**: vendored Ghostty terminal emulator (Zig source + C headers).
