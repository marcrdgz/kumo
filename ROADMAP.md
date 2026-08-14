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

## 🧬 0.3.0 — Daemon core 

> ✅ **Released** — `v0.3.0`, 2026-08-12.

- ✅ **State contract** (`src/state.rs`): serialize sessions, the layout tree, and
  per-pane identity (cwd, title, shell, AI program) into `state_dir()/state.json`.
  Versioned (`v1`), **atomic** write (tmp + rename), **tolerant** load (unknown
  version / corrupt JSON → fresh start, never crashes), pure data decoupled from
  the TUI.
- ✅ **tmux-style CLI**: `kumo` attaches to the last state if present (else fresh),
  `kumo attach` forces a restore, `kumo new [WORKSPACE]` starts fresh (never
  attaches), and `kumo [WORKSPACE]` remains a fresh-start alias for back-compat.
- ✅ **Daemon** owning the PTYs and terminal emulators; IPC socket in `runtime_dir`
  (`src/config.rs`) — the path the state contract reserved.
- ✅ **Rendering**: the daemon renders the whole UI headlessly and streams
  **dirty-row cell patches** to attached terminals; each terminal is a light
  renderer that draws cells and forwards input (wide chars handled). Re-attach
  is exact, and several terminals can watch the same session at once.
- ✅ `leader+d` detaches the terminal; the daemon keeps running. The daemon
  **auto-stops** when the last session closes — no lingering background process.
- ✅ `kumo attach` / `kumo ls` / `kumo kill` (protocol v1: framed bincode, full
  frames on attach/resize, row diffs otherwise).
- ✅ **`kumo new` creates a new session in the running daemon** via a `NewSession`
  IPC message; a fresh daemon still spawns with the session in the given
  workspace. Uses the client's cwd when no workspace is given.
- ✅ **Socket hygiene**: owner-only permissions (0o600) plus the same-owner
  check on every accepted connection (`SO_PEERCRED` on Linux, `getpeereid()` on
  macOS/BSD) — a different user's client is rejected, fail closed
  (`src/app/server.rs`).
- ✅ **Agents live in the daemon**: lifecycle detection, status, and audible
  alerts already run server-side (visible in the sidebar of any attached
  terminal). `kumo ls` surfaces each AI CLI's status (name + working/blocked/idle)
  so a blocked agent is noticeable from outside the TUI.
- ✅ **Update without losing the web** (final phase): `kumo update` swaps the
  binary and the daemon restarts **inheriting the live terminals** — running
  agents survive the update (screens come back fresh; a full screen+scrollback
  restore is deferred — see 0.6.0). The daemon execs the new binary in place
  (`daemon --resume`),
  adopting each pane's PTY master descriptor from a transient resume file;
  attached terminals auto-reconnect over the fresh socket.

## ⚙️ 0.4.0 — Config & keymaps

> ✅ **Released** — `v0.4.0`, 2026-08-13.

- ✅ **Config → TOML**: the flat `key = value` file (Ghostty-style) migrates to
  TOML so keymaps, themes, and status-bar widget lists get real structure and
  native types (arrays, tables, booleans). The flat format still reads as a
  fallback (same merge pattern as the legacy `~/.kumo`); the v1 schema that
  1.0 freezes is the TOML schema.
- ✅ **Custom leader keys**: the leader chord is configurable via
  `[keymap] leader = "ctrl+b"` (`src/config.rs`, parsed in `src/app/bindings.rs`),
  with clear fallback + warning on invalid values. Per-mode keymaps for
  **popups** (menu / context-menu / popup navigation) stay deliberately fixed —
  like tmux, only the command bindings are remappable.
- ✅ **Keymap data-driven**: the hard-coded `leader_command` dispatch
  (`src/app.rs`) is now a single keymap table in `src/app/bindings.rs` — each
  entry a dispatch chord + action, and the same table feeds the dispatch, the
  leader-mode hint, and the `leader+?` showcase, so they never drift. Bindings
  are remappable from `config.toml` (`[keymap.bindings]`, e.g.
  `s = "split-vertical"`), with invalid chords/actions ignored after a warning.
  The **missing stock bindings** landed too: **keyboard resize**
  (`leader+H/J/K/L`), swap-panes (`leader+s`), rotate-layout (`leader+o`), and
  show-pane-numbers (`leader+q`). Mouse gestures stay fixed: drag-resize, the
  context menu, and selection are positional hit-testing, not key sequences,
  so they are deliberately **not** remappable.
- ✅ **`[terminal]` section**: `shell` (canonical home; a top-level `shell`
  stays as a deprecated alias) plus the **`new-cwd` session working-directory
  policy** — `follow` (default, live), `home`, `current`, or `fixed` (with a
  `fixed-cwd` path). An explicit `kumo new [dir]` always wins over the policy.
- ✅ **Follow workspace**: with `new-cwd = "follow"`, the workspace follows the
  focused pane's actual cwd, so new panes open where you are and the sidebar /
  git-branch follow along. **Primary mechanism is PID-based** with zero shell
  setup: kumo asks **which process group controls the pane's terminal right
  now** (`tcgetpgrp` on the PTY master, falling back to `e_tpgid` via
  `proc_pidinfo` on macOS / `tpgid` in `/proc/<pid>/stat` on Linux) and reads
  the **foreground job leader's** cwd (`/proc/<pid>/cwd` on Linux,
  `proc_pidinfo(PROC_PIDVNODEPATHINFO)` with an `lsof` fallback on macOS).
  Because it tracks the foreground group — not the deepest process — a
  lingering background job never hijacks the reported location. **OSC 7 /
  OSC 9 / OSC 1337 is wired as a passive complement** (`pwd_changed` now
  enabled in `src/vt.rs`): shells that already emit OSC 7 (oh-my-zsh, kitty
  distros, fish, …) report their cwd directly, which is the only signal that
  works inside remote `ssh` panes. **ON by default**, `new-cwd = "follow"`, no
  leader binding. The one-shot **snippet installer** for shells that don't
  emit OSC 7 is deferred to 0.7.0 (with the command traceback work).
- ✅ **Config reload**: `kumo reload` (CLI) and the MENU `reload` item re-read
  the config and apply `shell`, `ai-cmd`, `leader`, and `keymap.bindings` live
  to panes spawned from then on. `new-cwd` and `agent-sound` apply instantly
  (read live on use). The auto-reload **file watcher** stays in 0.5.0.
- ✅ **MENU `config`** opens the config file in an editor pane (split) inside
  the session — `$VISUAL` → `$EDITOR` → `vi`, preferring `config.toml`.

**Deferred from 0.4.0**: full screen+scrollback restore after update/restart
(only processes and layout survive today; the lossy ANSI replay becomes
lossless with 0.6.0's copy-mode/scrollback work), OSC 133 semantic prompts
(only 0.7.0's command traceback consumes them), the **OSC 7 snippet installer**
(follow-workspace works without it; it only adds remote-ssh coverage — 0.7.0),
status-bar layout (lands whole with 0.5.0's widgets), and the control CLI /
scripting (`kumo send-keys`, `kumo split`, … — now 0.6.0).

## 🎨 0.5.0 — Theme & chrome

- **Themes**: full color palette customization (Catppuccin-style schemes,
  light/dark variants, per-scheme backgrounds, status-dot colors, borders) —
  named colors (`mauve`, `navy`) and the sidebar git-branch (with ahead/behind)
  already landed; this release turns them into a real theme engine with
  configurable values (today constants in `src/app.rs`, `src/app/ui.rs`).
- **Config hot-reload**: watch the config file and reload theme/config live —
  no restart to pick up changes; lands here so themes are instantly tweakable.
- **Status bar**: customizable widgets (branch, session, agent status, hostname,
  clock) — includes the status-bar **layout** config deferred from 0.4.0.
- **Sidebar**: toggle/order sections; pane titles and border styling. The
  SESSIONS and AGENTS sections become two full **tabs** in the panel (today two
  stacked, independently-scrolling regions in `src/app/sidebar.rs`).
- **Popup input editing**: `cmd+backspace` / `ctrl+backspace` (delete word)
  and `cmd+delete` / `ctrl+delete` (delete forward word) in the rename / new
  popups. Popup keymaps stay deliberately fixed (see 0.4.0) — this is editing
  nicety, not remapping.

## 🔍 0.6.0 — Copy-mode, search & pane plumbing

- **Tabs (windows) per session**: each session owns an ordered list of tabs,
  each tab its own named pane tree — one session can hold several workspaces /
  worktrees under a single leader. Adds the intermediate "window" level the
  flat model (sessions → panes) skipped; the layout tree, split keys, and the
  state contract `v1` (0.3.0) are extended here, before the 1.0 schema freeze.
- **Copy-mode**: vi-style keyboard selection over scrollback + `/` search — the
  biggest missing multiplexer feature (the scrollback already exists in ghostty;
  only the selection/search UI is missing).
- **Sync-input**: type into every pane at once.
- **Pipe-pane / logging**: capture a pane's output to a file.
- **Full screen+scrollback restore after update/restart**: today only the
  processes and layout survive `kumo update` (the re-encode-as-ANSI replay is
  lossy); this makes the screen + scrollback come back exactly, sharing the
  same scrollback machinery as copy-mode.
- **Control CLI / scripting** (`kumo send-keys`, `kumo split`, …): client
  commands over the daemon socket, driven by the same keymap tables (deferred
  from 0.4.0).

## 🤖 0.7.0 — Agent breadth & AI polish

- Lifecycle detection for `codex · gemini · qwen · aider · cody · swe · coco`
  (today auto-listed, always idle).
- Improved context sharing: scrollback → prompt, and command traceback — the
  **OSC 133** semantic-prompt boundaries (the snippet installer lands here with
  the traceback work) let the AI pane auto-attach "the last failing command +
  its output" without blind scrollback parsing.

## 🛡️ 0.8.0 — Stability

- Hardening of `SIGCHLD`/`SIGWINCH`, stable macOS + Linux CI, complete config
  docs, deprecation of legacy `~/.kumo`, published performance benchmark.
- Windows is **experimental** here; full parity is a post-1.0 (1.x) item so it
  never blocks 1.0.

## 🎉 1.0.0

Full customization + solid persistence, meeting the gate criteria above — the
**TOML** config schema is the frozen v1, macOS + Linux are first-class, Windows
stays experimental. The layout model — **sessions → tabs → panes**, tabs
landing in 0.6.0 — gets documented as an explicit design decision, so it never
resurfaces as a perpetual issue.

---

## 🧩 After 1.0 (1.x)

Planned: a **thin** plugin system so the community can add custom commands,
widgets, and integrations without forking kumo — deliberately kept out of the
1.0.0 scope, and deliberately light (widgets, commands, hooks) to keep the
featherweight story instead of a heavy runtime (à la zellij).

**Windows parity** — the gate moved out of 0.8.0: full parity, stable
cross-platform CI, and a Windows release build.

Beyond that, the differentiating bets:

- **Semantic session timeline / time-travel scrollback**: with OSC 133
  boundaries, the cwd signal from follow-workspace, and git, the scrollback
  becomes a structured, queryable timeline — "rewind a pane to just before the
  last command", jump to a command's output, search commands across every pane.
- **Kumo as an MCP server**: expose panes, sessions, and scrollback over MCP so
  *any* agent can drive kumo (split, send-keys, read output) — the natural
  evolution of the 0.6.0 control CLI. Agents stop just living inside the
  terminal and start orchestrating it.
- **Multi-agent supervisor**: not just detecting agents but coordinating them —
  an inbox that routes blocked approvals, a status summary across all agents,
  and a pane that watches another agent's output and reacts.
- **Session migration across machines**: extend the resume mechanism
  (`resume.json` + `daemon --resume`) into `kumo migrate`, moving a live
  session between hosts.
- **Declarative workspaces**: define a project's layout as TOML (panes +
  commands + cwd) and have kumo restore it on demand.
- **Session sharing** (tmate-style): a peer attaches to your daemon over a
  socket / SSH to pair-program; read-only observers for reviews.
- **Remote sessions**: mosh/SSH-style remote panes with a local control pane.
- **Deeper AI**: with OSC 133 boundaries + the git-branch from follow-workspace,
  the AI pane auto-attaches "last failing command + diff" with zero user action.

Quality-of-life ideas that round out the editor feel:

- **Command palette / fuzzy switcher** over sessions, actions, and keybinds.
- **System notifications** (macOS / notify-send) for blocked agents — today
  only an audible chime.
- **tmux control-mode compatibility** so existing tooling (neovim, scripts)
  keeps working.
- **Asciinema export**: record a pane's session to a file / stream.

---

Until then, kumo stays opinionated: it picks good defaults for you so it just
works, and you can follow the roadmap above as the knobs land. 🔧
