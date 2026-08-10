# 🗺️ Kumo Roadmap

The long-term goal is that **every aspect of kumo is configurable by you**:
colors, keybindings, the leader key, the layout of the status bar and sidebar,
the AI panes, and more. The current build ships with a sensible fixed
configuration, and the pieces below are planned — **not yet implemented**.

The roadmap below is versioned. Every `0.x.0` is a feature (minor) release via
`cargo release` + git-cliff; 1.0.0 is the finish line for the configurable,
persistent kumo.

## 🎯 Definition of 1.0.0

**1.0.0 = full customization + solid persistence.** Gate criteria:

1. Config schema **v1 frozen and documented** (with migrations if it changes).
2. Keymap stable — renaming bindings never breaks user configs.
3. Persistence and detach **without state loss** (crash-safe).
4. Theme system stable.
5. Release pipeline proven (stable + nightly), `cargo clippy`/`test` green in CI.

---

## 🚀 0.2.0 — Polish & hygiene

> ✅ **Shipped** — `leader+?` opens a browsable keybind showcase, and the
> leader-mode status-bar hint is generated from the same table
> (`src/app/bindings.rs`), so the two never drift.

- ✅ CLI `-h/--help` usage output
- ✅ Context menu with split / close actions
- ✅ Per-agent lifecycle detection split + `claude` support
- ✅ Mouse SGR forwarding fix; git-cliff changelog pipeline
- Rename the `d` binding from *detach* to *exit* until real detach lands
  (`src/app/bindings.rs`, `src/app.rs`, `src/app/overlays.rs`)

## ⚙️ 0.3.0 — Config & keymaps

- **Keymap data-driven**: the hard-coded leader dispatch (`leader_command` in
  `src/app.rs`) and mouse actions become configurable tables; remap any binding
  (splits, focus, sessions, zoom, sidebar, …).
- **Custom leader keys** and per-mode keymaps (normal / leader / popup).
- **Config expansion** (`src/config.rs`): `keymap`, `leader`, status-bar layout;
  **clear validation errors** instead of silent ignores.
- The `config` item in the MENU dropdown (today "coming soon") opens the config
  file for editing.

## 🎨 0.4.0 — Theme & chrome

- **Themes**: full color palette customization (Catppuccin-style schemes,
  light/dark variants, per-scheme backgrounds, status-dot colors, borders) —
  today hard-coded constants in `src/app.rs`, `src/app/ui.rs`.
- **Status bar**: customizable widgets (branch, session, agent status, hostname,
  clock).
- **Sidebar**: toggle/order sections; pane titles and border styling.

## 💾 0.5.0 — Session persistence (light restore)

- Serialize layout tree + sessions + cwd (plus scrollback via Ghostty
  serialization) into `state_dir`.
- Auto-restore on launch, with `kumo --resume` / `--no-resume`.
- Processes **restart** (fresh shells); restore is state, not live processes.
  → *Reaparecer sesiones y paneles como estaban* at the layout level.

## 🔌 0.6.0 — Detach real · client-server

- **Daemon** owning the PTYs; IPC socket in `runtime_dir` (`src/config.rs`).
- `kumo detach` / `kumo attach [session]` / `kumo ls` / `kumo kill`.
- Agents **survive** the TUI closing (covers *daemon para los agentes*).
- 0.5.0 + 0.6.0 together deliver full *reaparecer tal cual*.

## 🤖 0.7.0 — Agent breadth & AI polish

- Lifecycle detection for `codex · gemini · qwen · aider · cody · swe · coco`
  (today auto-listed, always idle).
- Improved context sharing: scrollback → prompt, command traceback.

## 🛡️ 0.8.0 — Stability & parity

- Windows parity, stable cross-platform CI, complete config docs, deprecation
  of legacy `~/.kumo`, hardening of `SIGCHLD`/`SIGWINCH`, published performance
  benchmark.

## 🎉 1.0.0

Full customization + solid persistence, meeting the gate criteria above.

---

## 🧩 After 1.0 (1.x)

Planned: a plugin system so the community can add custom commands, widgets, and
integrations without forking kumo. Deliberately kept out of the 1.0.0 scope.

---

Until then, kumo stays opinionated: it picks good defaults for you so it just
works, and you can follow the roadmap above as the knobs land. 🔧
