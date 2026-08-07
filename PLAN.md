# Neomux: Ghostty-powered Terminal Multiplexer with Claude AI Integration

## 1. Executive Summary
**Neomux** is a next-generation terminal multiplexer designed to combine high-performance terminal emulation (powered by Ghostty's core VT library) with deep native integration for AI assistance via Claude.

---

## 2. Core Architecture

### A. Terminal Emulation & Rendering (`libghostty`)
- Use Ghostty's core Zig engine (`libghostty`) for VT100/Xterm terminal emulation, ANSI escape parsing, and GPU-accelerated rendering.
- Spawn and manage PTY (pseudo-terminal) file descriptors attached to local shells (`zsh`, `bash`).

### B. Layout & Pane Manager
- **Hierarchical Layout Tree**: Manage panes using a split tree structure (horizontal & vertical splits).
- **Pane Management**: Focus switching, pane resizing, zooming, and swapping.
- **Session Persistence & Daemon**: Client-Server architecture allowing detaching (`ctrl+a d`) and reattaching sessions.

### C. Claude AI Deep Integration
- **Specialized AI Pane**: Dedicated UI split running an interactive Claude stream/agent.
- **Context Sharing**: Pipeline output, scrollback buffer, or recent command errors directly to Claude's prompt context.
- **Action Execution**: Allow Claude to suggest or execute verified commands directly inside target terminal panes.

---

## 3. High-Level Implementation Roadmap

1. **Phase 1: Ghostty Integration & PTY Shell**
   - Initialize project structure in Zig/C/Rust embedding `libghostty`.
   - Implement basic PTY spawning and raw input/output passthrough.

2. **Phase 2: Split Layout Engine & Keybindings**
   - Implement split-pane layout tree (binary tree of splits).
   - Implement keybinding dispatch system (e.g., Leader key `Ctrl+A` or `Ctrl+B`).

3. **Phase 3: Client-Server / Session Management**
   - Implement UNIX domain socket IPC to support persistent sessions and detach/reattach logic.

4. **Phase 4: Claude AI Agent Integration**
   - Build API client integration for Claude API / CLI.
   - Implement smart context extractions (e.g., send pane buffer, analyze error tracebacks).

5. **Phase 5: Polish & UI Customization**
   - Theme configuration, status line customization, modal menus, and keyboard shortcuts.

---

## 4. Agent Guidelines & English Technical Terminology

To ensure high-quality English technical output during development:
- **Terminal Concepts**: PTY (Pseudo-Terminal), TTY, Scrollback Buffer, VT100/Xterm Emulation, ANSI Escape Sequences, Raw Mode, Non-canonical Mode, IPC (Inter-Process Communication), Domain Sockets.
- **Multiplexer Terminology**: Leader Key, Split Tree, Panes, Windows, Sessions, Detach/Reattach, Grid Buffer.
- **Codebase Style**: Use clear, concise commit messages, complete docstrings, precise technical phrasing, and modular file organization.
