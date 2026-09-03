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
  (read live on use). The auto-reload **file watcher** stays in 0.7.0.
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
engine — today the schemes are picked, not user-editable), status-bar
widgets/layout, and sidebar section toggle/order + pane titles/border
styling; the config hot-reload file watcher rolls into 0.7.0.

## 🔍 0.6.0 — Copy-mode, search & pane plumbing

> ✅ **Released** — `v0.6.0`, 2026-08-22. (tmux's sync-input and pipe-pane were
> cut from this release: the broadcast prompt to agents (0.7.0) supersedes
> sync-input; pipe-pane moves to 0.9.0 under Asciinema/plugins.)

- ✅ **Theme engine** (deferred from 0.5.0): user-editable theme values on top
  of the 0.5.0 picker — full palette customization in `config.toml` (schemes,
  accents, status dots, borders) instead of the built-in constants.
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
- ✅ **Full screen+scrollback restore after update/restart**: the daemon now
  carries inline ghostty snapshots (`SavedPane.snapshot` in `app/kumo/src/daemon/state.rs:126`, `vt.rs: snapshot_encode`/`from_snapshot`, `pane.rs: finish_from_snapshot`) so `kumo update` and `daemon --resume` restore screen + scrollback exactly. Shipped in `v0.5.4` ("Preserve scrollback across restart via inline snapshot"); the earlier lossy ANSI-replay fallback is retired.
- ✅ **Control CLI / scripting** (`kumo session|tab|pane|agent`, `kumo pane send-keys`/`split`/`close`/`focus`, `kumo reload`): client commands over the daemon socket, driven by the same keymap tables (deferred from 0.4.0; `app/kumo/src/cli/cli.rs`, `app/kumo/src/daemon/app/server.rs:409`).

## 🤖 0.7.0 — ADE (AI Development Environment), AI Polish & Native Context

The ADE release. Two layers: **agent-native plumbing** (states, waits,
reporting — the coordination surface a good host has to have) and kumo's own
**context pipeline** (traceback + diffs +
verify — the part that makes kumo an ADE, not just a host). The agent surface
is **open**: any process in a pane can report its own state, and any agent can
drive kumo over the same JSON socket the CLI uses (`app/kumo/src/cli/cli.rs`,
`crates/kumo-protocol`).

**Agent state model v2**:
- ✅ **Five states** — `working · blocked · idle · done · unknown`
  (`app/kumo/src/daemon/agents/mod.rs`). `done` is *finished-but-unseen*: an
  agent that went idle while its pane/tab was not focused (it aligns with the
  existing 0.6.0 `finished` toast channel — toasts now announce the unseen
  finish; a focused finish stays silent). `unknown` is a recognized agent
  whose classification failed (Idle is now proven by explicit per-agent idle
  markers, not the no-signal fallback). The Inbox (below) and the sidebar
  rollup key off `done` — needing attention, not yet seen — and focusing a
  pane marks its agent seen.
- **Lifecycle detection for `codex · gemini · qwen · aider · cody · swe · coco`**
  (today auto-listed, always idle): the same detection path (screen markers /
  OSC title spinner) promotes each to a **first-class state** instead of a
  silent always-idle row — every supported agent at minimum reports working, so
  the idle fallback stays trustworthy. With the ✅ rule engine below, each one
  now needs only a bundled/user `agent-detection/<agent>.toml` capturing its
  real UI markers (iterate with `kumo agent explain`) — no kumo release.
- ✅ **`kumo agent explain`** + **pane-id discovery**: debug why a pane reads
  the state it does — every matched marker region-tagged (screen /
  form / footer / title, per agent), the precedence breakdown
  (`blocked > working > idle > unknown`), and the verdict reason chain
  (idle markers · unseen finish / done · seen after focus · dead pane ·
  not-an-AI-CLI default · no-signal fallback), evaluated live by the running
  daemon (`Command::AgentExplain` → `DaemonEvent::AgentExplain`, evidenced
  path in `app/kumo/src/daemon/agents/mod.rs`). Pane ids are discoverable via
  `kumo agent status`/`list`/`ls` (one line per AI CLI with its pane id),
  `kumo pane list [-t TAB]` (every pane, optional tab filter), and
  `kumo session list` agents rows (`pane N`) — `kumo agent explain [PANE]`
  defaults to the first AI pane of the active session. Panes are also
  addressable by **composite position** `s1:t2:p3` / `kumo:t2:p1` / `t2:p1`
  (1-based, resolved client-side against the session list; the daemon keeps
  the canonical `u64` id), and every CLI list/explain/status line prints the
  position next to the id.

**Agent orchestration primitives** — the *wait* half of the `send-keys` story;
waiting (not just injecting) is the actual ADE differentiator: tmux never had
it, and it's the core of agent-to-agent work.
- ✅ **`kumo agent wait <pane> --until blocked|done|idle|working`** (`--timeout`): server-owned,
  event-driven; pins the pane occupant so a process replacement can't satisfy
  the wait (`agent_replaced`); returns `agent_blocked` immediately when the pane is already
  blocked; timeout sweeps with `drain_dead_panes` (`app/kumo/src/daemon/app/waits.rs: server-owned registry`, polled after `tick()` in `server.rs`).
- ✅ **`kumo agent prompt <pane> <text>`** (`--wait` / `--timeout`): bracketed-paste
  aware submit (`ESC[200~` when `MODE_BRACKETED_PASTE`); `--wait` races submit + wait into one server-owned request
  (skips the two-hop race of a separate send + poll), and refuses to inject when
  the agent is already blocked (`app.rs: agent_prompt_inject`).
- ✅ **`kumo agent read <pane> --source visible|recent|detection|traceback`**: the
  daemon already owns the screen buffer, and ghostty's screen buffer **holds the
  alternate screen** — full-screen agent transcripts (claude / codex) read
  directly from the buffer, no mouse-scroll transcript scraping (`app.rs: pane_read_text`). With OSC 133
  markers the same read is **structured**: `--source traceback` returns the last
  marked prompt + its output block (the same data the compose-popup consumes; today falls back to `form`/120 rows).
- ✅ **`kumo agent start --kind <codex|…> --pane <id>`** `[-- <args>]` + **`agent rename`**: launches an
  agent in an existing shell pane and returns once detection shows it ready
  (`agent_not_ready` when it starts blocked); `agent rename` adds live aliases
  so scripts reference agents by name, not pane id (`commands.rs: start/rename`); `agent broadcast` fans a prompt via `send-keys` path (`--filter`).
- ✅ **`kumo pane wait-output <pane> --regex`**: one-shot output waiter (no
  polling, `regex` crate, `bad_regex` on compile error) — what the verify loop and parallel agents wait on; `agent read` + `wait-output` hold alt-screen intact.
- **Verify loop** (`leader+r`) reworks its routing: run the suite into a fresh
  split, `pane wait-output` on it, and only feed the failure back to the agent
  once a `passed|failed`-shaped result is actually there.

**Open agent state reporting** — screen heuristics can't know everything:
- **`pane.report_agent` / `pane.report_agent_session` / `pane.report_metadata`**
  over the socket: any process, hook, or plugin declares its own
  working/blocked/idle/done state, its native resume id (claude `--resume`,
  codex `resume`), and display metadata (titles, state labels, scoped tokens
  with TTLs).
- **`KUMO_AGENT` wrapper hint** (`KUMO_AGENT=claude fence -- claude`): tell the
  daemon which detection rules a wrapper-launched agent uses — opaque wrappers
  no longer break detection.
- ✅ **Per-agent detection rules** (`agent-detection/<agent>.toml`, user-dir
  override, same rule shapes as the built-ins — markers + OSC title patterns):
  third-party agents get accurate state without a kumo release. The built-in
  classifiers (claude, opencode) now live as bundled manifests
  (`app/kumo/src/daemon/agents/rules/*.toml`) compiled into the binary and
  evaluated by a small data-driven engine (`app/kumo/src/daemon/agents/rules.rs`);
  a `config_dir()/agent-detection/<id>.toml` replaces or adds an agent's rules,
  loaded at daemon start and re-read by `kumo reload` (invalid files are
  warned and skipped — detection never crashes). Deliberately **local-only** in
  0.7.0; remote manifest updates defer to 0.8.0.

**JSON surface for agents** — the bincode socket stays for TUI-fast paths;
agents get a machine layer:
- **NDJSON** request/response over the same socket + **`kumo api schema`**
  emitting the full JSON Schema (`--json`, `--output`) so tools / LLMs can
  introspect the protocol; all control commands also gain `--json`.
- **Agent skill file** (`kumo-agents.md`, installed with the binary): teaches
  claude / codex / opencode how to orchestrate kumo natively — spawn, wait,
  read, report. Agents stop firing keystrokes and start orchestrating. (The
  MCP server stays post-1.0; this layer is its foundation.)
- **CLI Environment Injection** (`KUMO_SOCKET_PATH`): expose the daemon socket
  to spawned AI processes so they drive their own workspace layouts natively;
  can be disabled in config.

**Context pipeline** — ⏸ **Dropped** (kept only `vt.rs:last_prompt_block` + `pane_read_text:Traceback` as fallback): chip aggregation (`leader+i`/`P` compose, `context.rs` diff dump) was speculative, `OSC133`-gated and noisy, and duplicated `agent read` + `wait` that `tmux` never had. Orca ADE wins by *waiting*, not pasting. See revised 0.7.1–0.8.0 below (terminal-owned, daemon-wait reuse).

- ✅ **Agent Inbox View** (on the v2 state model): one unified tab aggregating
  `blocked · done · running` with direct keyboard navigation to actionable
  panes (`leader+i` focuses the sidebar agent panel: `j`/`k` move, `Enter`
  jumps to the pane, `Esc`/`q` leave). The sidebar defaults to two stacked
  panels — **spaces** on top (sessions + branches), the **agent panel** below
  grouped by state with counts (idle · unknown dimmed as a tail) and inline
  `kind · space · pane` row labels — separated by a grey divider that starts
  at the exact middle and drags to resize either panel (both scroll
  independently). Clicking the panel's `grouped` descriptor flips it to the
  classic rank-sorted workspace rows (and back); the legacy two-tab toggle
  stays one config key away (`[sidebar] layout = "tabs" | "divided"`,
  default `divided`). Declarative view queries (filter/sort rules read live
  from config or plugins) defer to 0.9.0 with the plugin system.

**Also in 0.7.0**:
- **Broadcast prompt to agents** (`leader+B`, `kumo agent broadcast`): fan one
  prompt out to every AI pane in the tab/session over the existing `send-keys`
  wire path (`app/kumo/src/cli/cli.rs`), filterable by agent status; the TUI
  action reuses the prompt popup and lives in the data-driven bindings table
  (`app/kumo/src/cli/bindings.rs`), so it shows up in `leader+?` and the
  leader hint automatically. Replaces tmux's sync-input: same "drive many
  panes at once" need, without the stray-keystroke footgun of raw input
  mirroring. (moved here from 0.6.0)
- **Config hot-reload file watcher** (deferred from 0.5.0): watch the config
  file and reload theme/config live — extends the manual `kumo reload` (0.4.0)
  so themes are instantly tweakable without a restart. (moved here from 0.6.0)

**Deferred from 0.7.0**: declarative agent-view queries (→ 0.9.0 plugin
system), remote/update-checked agent-detection manifests (→ 0.8.0),
`sync-input` stays cut (broadcast supersedes it), `pipe-pane` stays 0.9.0.

> Context pipeline dropped — superseded by Orca-native scope below (all in 0.7.0). Only `vt.rs:last_prompt_block` + `pane_read_text:Traceback` remain as fallback.

**Revised 0.7.0 scope — Orca-native, daemon-owned (`binary, not an app`, `AGENTS.md:4` `libghostty-vt`, `PLAN.md:68` dumb viewport, `protocol v12:54`)**
*Why superseded:* chips were speculative (`DIFF_CAP 8K` dump, `bottom_text(80)` heuristic), `OSC133`-gated, and duplicated `agent read` + `wait` that `tmux` never had. Orca wins by *waiting* and *remembering*, not pasting. All three below ship in **0.7.0** in this PR.

- **Supervisor Inbox v2 + Socket-MCP `KUMO_SOCKET_PATH`:** `leader+I` queue `Blocked→Done→Working` with evidence preview (`agent read --source detection`, not dump) and one-key `a`pprove/`d`eny/`s`kip/`v`erify. `v` spawns harness split (`layout.rs:50` `V|H` 0.05) → `PaneWaitOutput --regex passed|failed` (`waits.rs:89`) → `AgentPrompt --wait`. Demand-driven via `tasks.rs:215` `should_alert` → `DaemonEvent::Toast:519`, survives `AGENT_TOAST_TIMEOUT 5s`. Socket injects `KUMO_SOCKET_PATH`+`KUMO_BIN_PATH` (+`KUMO_SESSION/PANE_ID`) into every `PtySpec` (`pty.rs:15`, `pane.rs:18`), gated by `config.toml [agent] env_injection=true`, speaks `NDJSON` on same `UnixSocket 0o600` (`server.rs:897` `SO_PEERCRED`), `kumo api schema --json` (`schemars`) so `claude/codex/opencode` self-drive over `ssh -L`. `ClientKind::Agent` (`protocol.rs:66`). Drops `leader+i/P` compose (`context.rs`, `ComposeState:128`) — freed `i/P`.

- **Semantic Timeline Vault:** per-pane ring `VecDeque<Record{prompt,output,exit_code,cwd via vt.rs:1327 pwd, git_rev,ts}>` (200/pane, `16K` cap) pushed on `on_pty_event:1125` when `vt.last_prompt_block:1823` completes (`has_semantic_prompt:1969`). `git_rev` async like `tasks.rs:62` branch cache. TUI `leader+;` fuzzy picker (reuses `WorktreePicker:272` rect + `Grid:295` preview) filter `/`, `Enter` jump (`CopyScrollTo:932`), `y` yank, `r` rerun (`AgentPrompt`). Persist in `state.rs:57` `STATE_VERSION 2→3` tolerant load (`state.rs:134`). CLI `kumo timeline list [--grep] | show <id> | rerun <id> -p <pane>`. Structured `exit_code` vs `lower.contains("error")` (`context.rs:84`).

- **Declarative Workspaces & Checkpoints:** promote `LayoutSpec` (`protocol.rs:621`) to `kumo workspace save <file.toml>` + `apply` (transactional via `PENDING_PANES:29`), `git_rev` meta, plus `kumo checkpoint save <name> | list | restore | diff` as `git worktree add -b` + `git stash` (`worktrees.rs:62`), rollback = `branch switch`. Replaces imperative `kumo new --ai --context` (`commands.rs:320`). TUI `MENU` `workspace save/apply`.

## 🛡️ 0.8.0 — Stability

- Hardening of `SIGCHLD`/`SIGWINCH`, stable macOS + Linux CI (`cargo clippy`/`test` green), complete config docs, deprecation of legacy `~/.kumo`.
- **Config diagnostics**: `kumo doctor` / `kumo config check` validates `config.toml` (TOML syntax, unknown keys, invalid leader/chords, duplicate bindings, bad `fixed-cwd`) and surfaces the "ignored after warning" cases that are silent today.
- **Keymap conflict detection**: duplicate chords across bindings warn and the last-wins rule is documented; covered by the diagnostics above and the 1.0 keymap-stability gate.
- **Agent-detection manifest refresh** (deferred from 0.7.0): `kumo reload`/`kumo update` refresh bundled `agent-detection/<agent>.toml` rules, with an **opt-in** background remote manifest check (default off — rules stay local-first and versioned).
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

- **Semantic session timeline / time-travel scrollback** (shipped in 0.7.0 as Timeline Vault): extends the vault to "rewind a pane to just before the last command", jump to a command's output, search commands across every pane.
- **Kumo as an MCP server**: expose panes, sessions, and scrollback over MCP so
  *any* agent can drive kumo (split, send-keys, read output) — the natural
  evolution of the 0.6.0 control CLI. Agents stop just living inside the
  terminal and start orchestrating it.
- **Multi-agent supervisor** (shipped in 0.7.0 as Inbox v2): extends to auto-routing, status summary across all agents, and a pane that watches another agent's output and reacts.
- **Session migration across machines**: extend the resume mechanism
  (`resume.json` + `daemon --resume`) into `kumo migrate`, moving a live
  session between hosts.
- **Declarative workspaces** (shipped in 0.7.0): hardened TOML/env + `git_rev` round-trip and `kumo workspace apply` over `ssh`.
- **Session sharing** (tmate-style): a peer attaches to your daemon over a
  socket / SSH to pair-program; read-only observers for reviews.
- **Remote sessions**: mosh/SSH-style remote panes with a local control pane.
- **Deeper AI**: with 0.7.0's supervisor/timeline/workspace landed, the remaining stretch is cross-pane context via `MCP`/`KUMO_SOCKET_PATH` — plus multi-agent session hygiene (restart an agent with its native resume id).

Quality-of-life ideas that round out the editor feel (most now targeted for `0.9.0` — see above) — any leftovers stay as 1.x polish.

---

Until then, kumo stays opinionated: it picks good defaults for you so it just
works, and you can follow the roadmap above as the knobs land. 🔧
