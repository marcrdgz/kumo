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

> ✅ **Released** — `v0.2.0`, 2026-08-10.

- ✅ CLI `-h/--help` usage output
- ✅ Context menu with split / close actions
- ✅ Per-agent lifecycle detection split + `claude` support
- ✅ Mouse SGR forwarding fix; git-cliff changelog pipeline
- ✅ Rename the `d` binding from *detach* to *exit* until real detach lands
  (`src/app/bindings.rs`, `src/app.rs`, `src/app/overlays.rs`)
- ✅ `leader+?` keybind showcase; the leader-mode status-bar hint is generated
  from the same table (`src/app/bindings.rs`), so the two never drift

## ⚙️ 0.3.0 — Config & keymaps

- **Keymap data-driven**: the hard-coded leader dispatch (`leader_command` in
  `src/app.rs`) and mouse actions become configurable tables; remap any binding
  (splits, focus, sessions, zoom, sidebar, …). While the dispatch becomes a
  table, the **missing stock bindings** land too: **keyboard resize**
  (`leader+H/J/K/L` — today only mouse-drag resize exists), swap/rotate panes,
  and show-pane-numbers (`leader+q`).
- **Custom leader keys** and per-mode keymaps (normal / leader / popup).
- **Config expansion** (`src/config.rs`): `keymap`, `leader`, status-bar layout;
  **clear validation errors** instead of silent ignores.
- The `config` item in the MENU dropdown (today "coming soon") opens the config
  file for editing.
- **Follow workspace** — the active session tracks the focused pane's directory:
  new panes open where you are, and the sidebar / git-branch / AI context follow
  along. **ON by default**, `follow-workspace = true` in the config (no leader
  binding; config-only). Delivered via **OSC 7** (`pwd_changed` already exists
  in `libghostty-vt`, not yet wired in `src/vt.rs`): on first use kumo offers to
  auto-install the OSC 7 snippet into your shell rc (zsh / bash / fish, with
  confirmation). The same snippet install also enables **OSC 133 (semantic
  prompts)** in one pass — command start/end boundaries plus exit code — the
  foundation 0.8.0's command traceback needs, no blind scrollback parsing. The
  snippet is **idempotent and reversible**, and skips shells that already emit
  OSC 7 (several distros / Oh My Zsh do).

## 🎨 0.4.0 — Theme & chrome

- **Themes**: full color palette customization (Catppuccin-style schemes,
  light/dark variants, per-scheme backgrounds, status-dot colors, borders) —
  today hard-coded constants in `src/app.rs`, `src/app/ui.rs`.
- **Config hot-reload**: watch the config file and reload theme/config live —
  no restart to pick up changes; lands here so themes are instantly tweakable.
- **Status bar**: customizable widgets (branch, session, agent status, hostname,
  clock).
- **Sidebar**: toggle/order sections; pane titles and border styling.

## 💾 0.5.0 — Session persistence (light restore)

- Serialize layout tree + sessions + cwd (plus scrollback via Ghostty
  serialization) into `state_dir`.
- Auto-restore on launch, with `kumo --resume` / `--no-resume`.
- Processes **restart** (fresh shells); restore is state, not live processes.
  → *Reaparecer sesiones y paneles como estaban* at the layout level.
- ⚠️ **Daemon-ready design**: 0.5.0 and 0.6.0 solve the same problem two ways —
  a decision is made explicitly before opening 0.5.0: either keep light restore
  but design the state format as a **contract the 0.6.0 daemon reuses**, or
  invert the order and build the daemon first (with a daemon, the crash-safe
  persistence of gate #3 comes almost for free). The format must not be
  rewritten when the daemon lands.

## 🔌 0.6.0 — Detach real · client-server

- **Daemon** owning the PTYs; IPC socket in `runtime_dir` (`src/config.rs`).
- `kumo detach` / `kumo attach [session]` / `kumo ls` / `kumo kill`.
- **Control CLI / scripting** (`kumo send-keys`, `kumo split`, `kumo new-session`, …)
  — nearly free once the IPC socket exists; turns kumo into an automation target
  (editors, test runners, shell scripts).
- Agents **survive** the TUI closing (covers *daemon para los agentes*).

## 🔍 0.7.0 — Copy-mode, search & pane plumbing

- **Copy-mode**: vi-style keyboard selection over scrollback + `/` search — the
  biggest missing multiplexer feature (the scrollback already exists in ghostty;
  only the selection/search UI is missing).
- **Sync-input**: type into every pane at once.
- **Pipe-pane / logging**: capture a pane's output to a file.

## 🤖 0.8.0 — Agent breadth & AI polish

- Lifecycle detection for `codex · gemini · qwen · aider · cody · swe · coco`
  (today auto-listed, always idle).
- Improved context sharing: scrollback → prompt, command traceback — powered by
  the **OSC 133** boundaries installed with the follow-workspace snippet (0.3.0),
  no blind scrollback parsing.

## 🛡️ 0.9.0 — Stability & parity

- Windows parity, stable cross-platform CI, complete config docs, deprecation
  of legacy `~/.kumo`, hardening of `SIGCHLD`/`SIGWINCH`, published performance
  benchmark.

## 🎉 1.0.0

Full customization + solid persistence, meeting the gate criteria above. The
deliberately **flat model** (sessions → pane tree, no intermediate "windows")
gets documented as an explicit design decision, so it never resurfaces as a
perpetual issue.

---

## 🧩 After 1.0 (1.x)

Planned: a plugin system so the community can add custom commands, widgets, and
integrations without forking kumo. Deliberately kept out of the 1.0.0 scope.

Beyond plugins:

- **Session sharing** (tmate-style): a peer attaches to your daemon over a
  socket / SSH to pair-program.
- **Remote sessions**: mosh/SSH-style remote panes with a local control pane.
- **Asciinema export**: record a pane's session to a file / stream.
- **Deeper AI**: with OSC 133 boundaries + the git-branch from follow-workspace,
  the AI pane auto-attaches "last failing command + diff" with zero user action.

---

Until then, kumo stays opinionated: it picks good defaults for you so it just
works, and you can follow the roadmap above as the knobs land. 🔧
