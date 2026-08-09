# 🕷️ Kumo

> **蜘蛛** — *spider* in Japanese.

**Kumo** is a terminal multiplexer that weaves your AI agents together into a
single **web** 🕸️. Every pane is a live terminal, every AI CLI you run
(`opencode`, `claude`, `codex`, …) is a thread in that web, and the sidebar
keeps you aware of what each agent is doing at a glance — because a spider
always knows what's happening in its web. ✨

Built in **Rust** 🦀 with a single TUI frontend on **ratatui**, real terminal
emulation from **libghostty-vt** (Ghostty's headless terminal core), and
**portable-pty** for PTY management. 🧵

## ✨ Features

- 🖥️ **Real terminal emulation** — each pane is a genuine VT/xterm emulator
  (vendored Ghostty). Shells, TUIs, and full-screen apps behave exactly like in
  a native terminal: cursor, colors, scrollback, the works. 🎨
- 🧩 **Split panes & sessions** — binary split tree (vertical/horizontal),
  mouse-drag resizing, zoom 🔍, multiple independent sessions, tab cycling.
- 🤖 **AI CLI panes** — spawn a dedicated pane running your AI CLI with one
  keystroke, or just run `opencode` / `claude` / `codex` … in any shell pane:
  kumo auto-detects the agent process and weaves it into the sidebar. 🕸️
- 🟢🟠⚪ **Agent status at a glance** — the sidebar shows each agent's workspace
  and CLI name with a status dot: **green** = working, **orange** = blocked
  (waiting for approval), **gray** = idle. Detected from the terminal's live
  screen, so it updates instantly. ⚡
- 📋 **Native text selection** — drag to select in any pane, even inside apps
  that own the mouse (opencode's TUI, vim, less). Clicks still reach the app,
  and the highlight hugs the text like a normal terminal. ✂️
- 🐣 **Plain-tty identity** — panes present as a plain `xterm-256color` (no
  mouse capabilities advertised), so apps don't hijack the mouse and selection
  always works. 🧤
- 📂 **Workspace-aware** — kumo opens in the directory you launch it from; every
  new pane/session starts there. 🗂️

## 🛠️ Requirements

- 🦀 Rust toolchain (`cargo`)
- ⚡ [Zig](https://ziglang.org) on `PATH` — `build.rs` compiles the vendored
  `libghostty-vt` library at build time

## 📦 Install

```sh
cargo install --path . --locked
# or: make install
```

This builds the whole crate and installs the `kumo` binary. 🚀

## ▶️ Run

```sh
kumo            # 🏠 opens in the current directory
kumo ~/proyecto # 🗂️ opens in a specific workspace
kumo --version  # ℹ️ prints the version
```

During development: `cargo run -p kumo` or `make run`. 🧑‍💻

## ⌨️ Keybindings

| Key | Action |
| --- | --- |
| `Ctrl+Space` | Leader key (shows all bindings in the status bar) |
| `v` / `-` | Vertical / horizontal split 🪓 |
| `a` | AI CLI pane (vertical split) 🤖 |
| `c` | New session 🆕 |
| `x` | Close focused pane ❌ |
| `z` | Zoom pane 🔍 |
| `h`/`j`/`k`/`l` | Move focus left / down / up / right 🎯 |
| `n` / `p` | Cycle session next / previous ⏭️ |
| `Tab` | Cycle pane ↹ |
| `b` | Toggle sidebar 📌 |
| `d` | Detach (exit the TUI) 🚪 |
| `Esc` | Exit leader mode ↩️ |

**🖱️ Mouse**

- 🖐️ **Drag** selects text (copied on release) — works even in apps with mouse
  reporting.
- 👆 **Click** is forwarded to the app when it owns the mouse.
- 🎚️ **Scroll** is forwarded to reporting apps; otherwise it scrolls the pane's
  scrollback.
- ↔️ **Drag a splitter** to resize.

## 🤖 AI CLI configuration

By default the AI pane runs `opencode`. To use another agent:

```sh
echo 'ai_cmd=claude' >> ~/.kumo
# or via env var
KUMO_AI_CMD="claude --model sonnet" kumo
```

Detected agents (auto-listed in the sidebar when running in any pane):
`opencode` 🧠 · `claude` 💬 · `codex` ⚙️ · `gemini` ✨ · `qwen` 🐉 · `aider` 🛠️ ·
`cody` 🐶 · `swe` 🔧 · `coco` 🥥

## 🗂️ Project layout

```
Cargo.toml              📄 workspace + package manifest
build.rs                🏗️ compiles the vendored libghostty-vt (Zig)
src/main.rs             🚪 TUI entry point
src/app.rs              🧩 sessions/panes, layout tree, input routing, mouse, rendering
src/pane.rs             🪟 Pane = PTY + libghostty-vt terminal (agent status, dirty render)
src/vt.rs               🔌 FFI bindings to libghostty-vt (emulator + native selection)
src/pty.rs              🔧 portable-pty wrapper
src/config.rs           ⚙️ shell / AI CLI resolution and ~/.kumo config
src/xtgettcap.rs        🧤 plain-tty capability responder
vendor/libghostty-vt/   📚 vendored Ghostty terminal emulator (Zig + C headers)
```

---

🕸️ *A spider always knows what's happening in its web.*
