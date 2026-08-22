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

1. Config schema **v1 frozen and documented** (with migrations if it changes; every state bump ships a round-trip migration test — see `app/kumo/src/daemon/state.rs: v1→v2`).
2. Keymap stable — renaming bindings never breaks user configs; duplicate chords are detected and warned instead of silently shadowing.
3. Persistence and detach **without state loss** (crash-safe: atomic `tmp+rename` writes in `state.rs: save`, tolerant load on corrupt/unknown version, `kill -9` restore verified).
4. Theme system stable.
5. Release pipeline proven (stable + nightly), `cargo clippy`/`test` green in CI, with published benchmarks (attach latency, memory/pane, keystroke→render — the `4 ms` daemon / `8 ms` client budget from `v0.5.2`).

---

## 🚀 0.2.0 — Polish & hygiene

> ✅ **Released** — `v0.2.0`, 2026-08-10.

- ✅ CLI `-h/--help` usage output
- ✅ Context menu with split / close actions
- ✅ Per-agent lifecycle detection split + `claude` support
- ✅ Mouse SGR forwarding fix; git-cliff changelog pipeline
- ✅ Rename the `d` binding from *detach* to *exit* until real detach lands
  (`app/cli/src/bindings.rs`, `app/daemon/src/app.rs`, `app/cli/src/client_view.rs`)
- ✅ `leader+?` keybind showcase; the leader-mode status-bar hint is generated
  from the same table (`app/cli/src/bindings.rs`), so the two never drift

## 🧬 0.3.0 — Daemon core 

> ✅ **Released** — `v0.3.0`, 2026-08-12.

- ✅ **State contract** (`app/daemon/src/state.rs`): serialize sessions, the layout tree, and
  per-pane identity (cwd, title, shell, AI program) into `state_dir()/state.json`.
  Versioned (`v1`), **atomic** write (tmp + rename), **tolerant** load (unknown
  version / corrupt JSON → fresh start, never crashes), pure data decoupled from
  the TUI.
- ✅ **tmux-style CLI**: `kumo` attaches to the last state if present (else fresh),
  `kumo attach` forces a restore, `kumo new [WORKSPACE]` starts fresh (never
  attaches), and `kumo [WORKSPACE]` remains a fresh-start alias for back-compat.
- ✅ **Daemon** owning the PTYs and terminal emulators; IPC socket in `runtime_dir`
  (`crates/kumo-core/src/config.rs`) — the path the state contract reserved.
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
  (`app/daemon/src/app/server.rs`).
- ✅ **Agents live in the daemon**: lifecycle detection, status, and audible
  alerts already run server-side (visible in the sidebar of any attached
  terminal). `kumo ls` surfaces each AI CLI's status (name + working/blocked/idle)
  so a blocked agent is noticeable from outside the TUI.
- ✅ **Update without losing the web** (final phase): `kumo update` swaps the
  binary and the daemon restarts **inheriting the live terminals** — running
  agents survive the update (screen + scrollback now restored via inline
  snapshot — see 0.6.0 / `v0.5.4`). The daemon execs the new binary in place
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
  `[keymap] leader = "ctrl+b"` (`crates/kumo-core/src/config.rs`, parsed in `app/cli/src/bindings.rs`),
  with clear fallback + warning on invalid values. Per-mode keymaps for
  **popups** (menu / context-menu / popup navigation) stay deliberately fixed —
  like tmux, only the command bindings are remappable.
- ✅ **Keymap data-driven**: the hard-coded `leader_command` dispatch
  (`app/daemon/src/app.rs`) is now a single keymap table in `app/cli/src/bindings.rs` — each
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
  enabled in `app/daemon/src/vt.rs`): shells that already emit OSC 7 (oh-my-zsh, kitty
  distros, fish, …) report their cwd directly, which is the only signal that
  works inside remote `ssh` panes. **ON by default**, `new-cwd = "follow"`, no
  leader binding. The one-shot **snippet installer** for shells that don't
  emit OSC 7 is deferred to 0.7.0 (with the command traceback work).
- ✅ **Config reload**: `kumo reload` (CLI) and the MENU `reload` item re-read
  the config and apply `shell`, `ai-cmd`, `leader`, and `keymap.bindings` live
  to panes spawned from then on. `new-cwd` and the `[notifications]` knobs
  (`position`, `sound`) apply instantly
  (read live on use). The auto-reload **file watcher** stays in 0.6.0.
- ✅ **MENU `config`** opens the config file in an editor pane (split) inside
  the session — `$VISUAL` → `$EDITOR` → `vi`, preferring `config.toml`.

**Deferred from 0.4.0**: ~~full screen+scrollback restore after update/restart~~ landed in `v0.5.4` (inline snapshot; see 0.6.0), ~~control CLI / scripting~~ landed in 0.6.0 (`kumo session|tab|pane|agent`, `pane send-keys`/`split`); remaining deferred are OSC 133 semantic prompts (only 0.7.0's command traceback consumes them), the **OSC 7 snippet installer** (follow-workspace works without it; it only adds remote-ssh coverage — 0.7.0), and status-bar layout (lands whole with 0.6.0's widgets).

## 🎨 0.5.0 — Theme & chrome

> ✅ **Released** — `v0.5.0`, 2026-08-16.

- ✅ **Theme picker**: a settings popup with tabs (themes, …) replaces the
  fixed chrome — Catppuccin-style schemes, light/dark variants, per-scheme
  backgrounds, status-dot colors, borders. Named colors (`mauve`, `navy`) and
  the sidebar git-branch (with ahead/behind) are wired through the theme's
  secondary accent instead of hardcoded constants.
- ✅ **Sidebar tabs**: SESSIONS and AGENTS are two full **tabs** in the panel —
  a tab bar (click to switch, active highlighted) with per-tab scrollbars
  (`app/cli/src/client_view.rs`), active-row highlight across the full width,
  and clickable git-branch rows.
- ✅ **Popup input editing**: `cmd+backspace` / `ctrl+backspace` (delete word)
  and `cmd+delete` / `ctrl+delete` (delete forward word) in the rename / new
  popups. Popup keymaps stay deliberately fixed (see 0.4.0) — this is editing
  nicety, not remapping.
- ✅ **Git worktrees**: create and open git worktrees straight from the
  sessions list.
- ✅ **Clickable links**: open URLs on modifier+click, underlined while the
  modifier is held.
- ✅ **Selection & copy feedback**: the selection stays highlighted while
  copying, with a right-aligned copy confirmation in the status bar.
- ✅ **Chrome tuning**: wider pane left gutter, sidebar slimmed by one column;
  repaint and mouse-tracking fixes on resumed panes.

**Deferred from 0.5.0** into 0.6.0: configurable theme values (the theme
engine — today the schemes are picked, not user-editable), the config
hot-reload file watcher, status-bar widgets/layout, and sidebar section
toggle/order + pane titles/border styling.

## 🔍 0.6.0 — Copy-mode, search & pane plumbing

> 🚧 **In progress** — theme engine, status-bar widgets, sidebar polish, tabs, and copy-mode have landed; scrollback restore and control CLI followed in `v0.5.4` / `v0.5.2`. Remaining: file-watcher hot-reload, broadcast prompt to agents. (tmux's sync-input and pipe-pane were cut from this release: broadcast send-keys supersedes sync-input; pipe-pane moves to 0.9.0 under Asciinema/plugins.)

- ✅ **Theme engine** (deferred from 0.5.0): user-editable theme values on top
  of the 0.5.0 picker — full palette customization in `config.toml` (schemes,
  accents, status dots, borders) instead of the built-in constants.
- **Config hot-reload** (deferred from 0.5.0): watch the config file and
  reload theme/config live — extends the manual `kumo reload` (0.4.0) so
  themes are instantly tweakable without a restart.
- ✅ **Status bar widgets** (deferred from 0.5.0): customizable widgets (branch,
  session, agent status, hostname, clock) — includes the status-bar **layout**
  config deferred from 0.4.0. Configurable via `[status_bar]` (`left`/`center`/`right`
  widget lists, `enabled`, `[status_bar.widgets.*]` for clock/branch/agent/hostname/session)
  with live `kumo reload` and per-minute clock tick; collapses to `0` rows when
  `enabled = false` (`crates/kumo-core/src/config.rs`, `app/kumo/src/cli/status_bar.rs`,
  `app/kumo/src/cli/client_view.rs`).
- ✅ **Sidebar polish** (deferred from 0.5.0): toggle/order sections, pane
  titles, and border styling.
- ✅ **Tabs (windows) per session**: each session owns an ordered list of tabs,
  each tab its own named pane tree — one session can hold several workspaces /
  worktrees under a single leader. Adds the intermediate "window" level the
  flat model (sessions → panes) skipped; the layout tree, split keys, and the
  state contract `v1` (0.3.0) are extended here, before the 1.0 schema freeze.
- ✅ **Copy-mode**: vi-style keyboard selection over scrollback + `/` search — the
  biggest missing multiplexer feature (the scrollback already exists in ghostty;
  only the selection/search UI is missing).
- ✅ **Corner-toast notifications for blocked agents** (pulled forward from 0.9.0;
  desktop notifications were retired at `v0.6.0` — too invasive — so the
  channel is now a transient in-TUI **corner toast** on
  working→blocked (and idle/done) transitions, alongside the audible chime):
  the daemon pushes a `DaemonEvent::Toast` (`kumo-protocol`)
  from the same server-side detection + rate-limit site as the chime
  (`app/kumo/src/daemon/app/tasks.rs`; both channels share one per-pane
  cooldown), configured under `[notifications]` — **on by default**;
  `position = "off"` (or `KUMO_NO_NOTIFY=1`) silences toasts, per-channel
  `blocked` / `finished` pick which transitions notify, `sound = false` (or
  the deprecated top-level `agent-sound`, or `KUMO_NO_SOUND=1`) mutes the
  chime; all read live by `kumo reload`. Each viewer draws the
  toasts anchored by `[notifications] position` — `top-right` (default),
  `top-left`, `bottom-right`, `bottom-left`, `center`, or `never`/`off`
  (stacked, ~5 s lifetime, click-to-focus the agent's pane) —
  `app/kumo/src/cli/client_view.rs`.
- **Broadcast prompt to agents** (`leader+B`, `kumo agent broadcast`): fan one
  prompt out to every AI pane in the tab/session over the existing `send-keys`
  wire path (`app/kumo/src/cli/cli.rs`), filterable by agent status; the TUI
  action reuses the prompt popup and lives in the data-driven bindings table
  (`app/kumo/src/cli/bindings.rs`), so it shows up in `leader+?` and the
  leader hint automatically. Replaces tmux's sync-input: same "drive many
  panes at once" need, without the stray-keystroke footgun of raw input
  mirroring.
- ✅ **Full screen+scrollback restore after update/restart**: the daemon now
  carries inline ghostty snapshots (`SavedPane.snapshot` in `app/kumo/src/daemon/state.rs:126`, `vt.rs: snapshot_encode`/`from_snapshot`, `pane.rs: finish_from_snapshot`) so `kumo update` and `daemon --resume` restore screen + scrollback exactly. Shipped in `v0.5.4` ("Preserve scrollback across restart via inline snapshot"); the earlier lossy ANSI-replay fallback is retired.
- ✅ **Control CLI / scripting** (`kumo session|tab|pane|agent`, `kumo pane send-keys`/`split`/`close`/`focus`, `kumo reload`): client commands over the daemon socket, driven by the same keymap tables (deferred from 0.4.0; `app/kumo/src/cli/cli.rs`, `app/kumo/src/daemon/app/server.rs:409`).

## 🤖 0.7.0 — Agent breadth & AI polish

- Lifecycle detection for `codex · gemini · qwen · aider · cody · swe · coco`
  (today auto-listed, always idle).
- Improved context sharing: scrollback → prompt, and command traceback — the
  **OSC 133** semantic-prompt boundaries (the snippet installer lands here with
  the traceback work) let the AI pane auto-attach "the last failing command +
  its output" without blind scrollback parsing.

## 🛡️ 0.8.0 — Stability

- Hardening of `SIGCHLD`/`SIGWINCH`, stable macOS + Linux CI (`cargo clippy`/`test` green), complete config docs, deprecation of legacy `~/.kumo`.
- **Config diagnostics**: `kumo doctor` / `kumo config check` validates `config.toml` (TOML syntax, unknown keys, invalid leader/chords, duplicate bindings, bad `fixed-cwd`) and surfaces the "ignored after warning" cases that are silent today.
- **Keymap conflict detection**: duplicate chords across bindings warn and the last-wins rule is documented; covered by the diagnostics above and the 1.0 keymap-stability gate.
- **State migration tests**: every state-schema bump ships a round-trip save/load test (`app/kumo/src/daemon/state.rs: save_load_roundtrip`, `v1_migrates_to_single_tab`) and the tolerant load still fails closed to a fresh start on unknown/corrupt versions.
- **Crash-safety harness**: `kill -9` the daemon mid-write / mid-`state.json` `tmp+rename` and verify exact restore; exercises the atomic write (`state.rs: save`) and the tolerant load path. fsync policy documented.
- **Published performance benchmark** with concrete targets: attach latency, memory per pane (idle + scrollback), and keystroke→render (the `4 ms` daemon / `8 ms` client budget from `v0.5.2`); tracked in CI so regressions are visible.
- Windows is **experimental** here; full parity is a post-1.0 (1.x) item so it
  never blocks 1.0.

## ✨ 0.9.0 — QoL & plugins (RC)

Tightens the last gaps before the 1.0 freeze — **not a gate**, just polish so
1.0 feels complete. Scope stays small so 0.9.0 remains a release candidate.

- **Plugin system** (pulled forward from 1.x): shareable workflow packages — a directory with a `kumo-plugin.toml` manifest the host validates, then launches its argv commands (bash/js/lua/rust/binary — whatever `argv` can run). The plugin owns language, deps, and files; kumo owns install, manifest validation, keybindings, panes, events, invocation context, and socket access. No SDK or WASM runtime: the **entire `kumo` CLI is the plugin API** (`KUMO_BIN_PATH` + the socket), so anything you can run as `kumo ...` a plugin can run. Keeps the featherweight story — same reason we called it "thin" (vs. zellij's runtime).
  - **Manifest** (`kumo-plugin.toml`): `[plugin]` header — `id`/`name`/`version`/`api` (plugin-API **generation, int**, host refuses unknown ones)/`kumo-min`/`platforms`/`tags` —, optional `[build] steps` (argv, run once on `add`, never on `dev`), and **one unified `[[entry]]` table** whose `kind` selects the surface: `command` (palette/menu/keys/`run`), `trigger` (`on` daemon events), `boot` (one-shot after resume + socket ready), `pane` (`place` = `tab`/`split`), and link interception is a `command` entry with a `link` regex. Entry ids qualify as `<plugin-id>.<entry-id>`; validated on `add`/`dev` (id charset, uniqueness, `api`/platform gating, regex compile), and `dev` links warn when top-level `platforms` is missing.
  - **Distribution**: `kumo plugin add owner/repo[/subdir]` — **prefers GitHub Release tarballs by semver tag, falls back to git clone**; live preview + confirmation in interactive terminals (`--yes`/`--ref` for CI); refuses to install over a dev link. `dev`/`undev` for local checkouts (no build run), `rm`/`ls`/`run`/`logs`/`where`/`upgrade` (semver-aware, `--all`, re-add to refresh pinned tarball)/`search`/`check`. A committed `plugins.lockfile` pins source/ref/version/checksum (`--frozen` for CI); registry state (managed checkout vs dev link) lives under `data_dir()`. Marketplace auto-indexed from the `kumo-plugin` GitHub topic — no submission form.
  - **Runtime**: host injects `KUMO_BIN_PATH`, `KUMO_SOCKET_PATH`, `KUMO_PLUGIN_ID/ROOT/CONFIG_DIR/STATE_DIR` (state dir is **host-managed** — durable state is a first-class contract, not an afterthought) and `KUMO_INVOCATION_JSON` (source, ids, clicked_url, …); plugins call back over the CLI or the raw socket; plain argv — no shell expansion. Events: `agent.working|blocked|idle|done`, `pane.created|closed|focused`, `session.created|removed`, `tab.created|removed`, `worktree.created`, `config.reloaded`, `boot` — fired by an async daemon-side dispatcher at the state-transition sites; failures are logged, never fatal.
  - **Keys & links**: plugins never declare chords — **users** wire plugin entries in their own `[keymap.bindings]` (`l = { plugin = "…", entry = "…" }`), visible in `leader+?`; no chord squatting. Link handlers intercept modified clicks before `open_url`, in manifest order.
  - **Marketplace site**: minimal static site — landing, `/docs/plugins/` authoring guide (trust model + event catalog), `/plugins` grid whose cards show **version/platforms/tags** (the index **parses manifests**, not just repo metadata) plus stars/language/last-push; a scheduled Action every 30 min indexes public repos tagged `topic:kumo-plugin` (no forks/archived) into a committed `index.json`; `kumo plugin search` reads the same index. Source resolution sits behind a trait from day one so a hosted/Docker-Hub-style registry can bolt on without breaking v1. Seed a `kumo-plugin-examples` cookbook. Trust model unchanged: plugins run as your user — install from sources you trust, preview and skim manifests first.
  - **Build order (phases)**: (0) manifest + validation + registry/lockfile + `add/rm/ls/dev/undev/check` → (1) protocol variants + async runner + env injection + `run/logs/where` + surface in context menu/palette → (2) trigger dispatcher at event sites + `boot` → (3) keybinding side-table + link interception → (4) `pane` entries (`tab`/`split`) → (5) site + indexer + `search` + examples repo. Each phase lands with `cargo test` + `cargo clippy` and an end-to-end `add → run` test against a fixture git repo.
- **Command palette / fuzzy switcher** over sessions, actions (including plugin actions), and keybinds.
- **tmux control-mode compatibility** so existing tooling (neovim, scripts) keeps working.
- **Asciinema export**: record a pane's session to a file / stream. Subsumes
  tmux-style **pipe-pane** (cut from 0.6.0): capturing a pane's output to a
  file lands as this richer export plus a pane-output stream plugins can
  consume over the socket, not as a separate daemon tap.
- **Configurable scrollback limit** (`[terminal] scrollback-limit` / `scrollback-limit-lines`): expose ghostty's cap (`vt.rs: Terminal::new(max_scrollback)`, today hard-coded `10_000` in `app/kumo/src/daemon/pane.rs:323`) as optional `config.toml` keys. QoL only — **not** a 1.0 gate; the default stays `10_000` and the v1 schema can add it compatibly later. See also `vendor/libghostty-vt/src/config/Config.zig: scrollback-limit-bytes`.

## 🎉 1.0.0

Full customization + solid persistence, meeting the gate criteria above — the
**TOML** config schema is the frozen v1, macOS + Linux are first-class, Windows
stays experimental. The layout model — **sessions → tabs → panes**, tabs
landing in 0.6.0 — gets documented as an explicit design decision, so it never
resurfaces as a perpetual issue. `0.9.0` is the RC that proves the plugin + QoL surface is shippable before freezing.

---

## 🧩 After 1.0 (1.x)

With the plugin system shipped in `0.9.0`, After 1.0 extends it rather
than introducing it — plus the bigger bets below.

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

Quality-of-life ideas that round out the editor feel (most now targeted for `0.9.0` — see above) — any leftovers stay as 1.x polish.

---

Until then, kumo stays opinionated: it picks good defaults for you so it just
works, and you can follow the roadmap above as the knobs land. 🔧
