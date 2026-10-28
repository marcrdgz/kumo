# 🗺️ Kumo Roadmap

The long-term goal is that **every aspect of kumo is configurable by you**:
colors, keybindings, the leader key, the layout of the status bar and sidebar,
the AI panes, and more. This file tracks what is next; shipped work is in
[CHANGELOG.md](CHANGELOG.md).

The roadmap below is versioned. Every `0.x.0` is a feature (minor) release via
`cargo release` + git-cliff; 1.0.0 is the finish line for the configurable,
persistent kumo.

## 🎯 Definition of 1.0.0

**1.0.0 = full customization + solid persistence.** Gate criteria:

1. Config schema **v1 frozen and documented** (with migrations if it changes; every state bump ships a round-trip migration test — see `app/kumo/src/daemon/state.rs: v1→v2`).
2. Keymap stable — renaming bindings never breaks user configs; duplicate chords are detected and warned instead of silently shadowing.
3. Persistence and detach **without state loss**, including across a reboot (crash-safe: atomic `tmp+rename` writes in `state.rs: save`, tolerant load on corrupt/unknown version, `kill -9` restore verified).
4. Theme system stable.
5. Release pipeline proven (stable + nightly), `cargo clippy`/`test` green in CI, with a **measured, documented performance baseline** against the `4 ms` daemon / `8 ms` client budget from `v0.5.2` (CI-tracked benchmarks are 1.x).
6. Wire protocol **compatible across additive changes**: a major/minor split (or capability negotiation) replaces today's strict `PROTOCOL_VERSION` equality (`crates/kumo-protocol/src/lib.rs`), so a new command or event no longer locks out `kumo-desktop` or future remote clients; only breaking changes bump the major.
7. Control CLI is a **stable machine API**: `--json` output shapes, error codes, and exit codes are frozen and covered by golden tests — agents already drive kumo through it.
8. **Upgrade without session loss**: `kumo update` from the last 0.x release resumes running sessions on 1.0 (`resume.json` + `state.json`), verified against fixtures written by the previous release.
9. A written **compatibility policy** states what semver covers for each surface (config, keymap action names, CLI JSON, protocol, `state.json`) and how long deprecations last.

Plugins are deliberately **not** part of 1.0: they are a new public API surface and land in 2.0.0 (see below), so 1.0 freezes only what kumo already ships.

---

## ✅ Released

What each release shipped is in [CHANGELOG.md](CHANGELOG.md); the full
pre-1.0 plans live in git history.

- **0.7.0** — ADE: orchestration & isolated worktrees · `v0.7.0` 2026-09-13
  (patches through `v0.7.6`)
- **0.6.0** — Copy-mode, search & pane plumbing · `v0.6.0` 2026-08-22
- **0.5.0** — Theme & chrome · `v0.5.0` 2026-08-16
- **0.4.0** — Config & keymaps · `v0.4.0` 2026-08-13
- **0.3.0** — Daemon core · `v0.3.0` 2026-08-12
- **0.2.0** — Polish & hygiene · `v0.2.0` 2026-08-10

## 🧭 Design decisions

Choices made along the way that should not be re-litigated without a new
reason:

- **Sessions → tabs → panes** is the layout model (tabs since 0.6.0); there is
  no further nesting level.
- **Only command bindings are remappable.** Popup/menu keymaps stay fixed, like
  tmux, and mouse gestures (drag-resize, context menu, selection) are
  positional hit-testing, not key sequences.
- **Follow-workspace reads the foreground process group's cwd** (zero shell
  setup), so a lingering background job never hijacks the location; OSC 7 is a
  passive complement and the only signal inside remote `ssh` panes.
- **The socket is owner-only and fails closed**: `0o600` plus a same-owner peer
  check on every connection.
- **Agent state comes from data-driven detection manifests**
  (`agent-detection/<agent>.toml`), local-first: a new agent needs a manifest,
  not a kumo release, and invalid files never crash detection.
- **Notifications are in-TUI corner toasts plus a chime.** Desktop
  notifications were retired at `v0.6.0` as too invasive.
- **Broadcast replaces sync-input**: `kumo agent broadcast` fans one prompt to
  AI panes instead of mirroring raw keystrokes; tmux-style `pipe-pane` becomes
  the 1.x Asciinema export.
- **Worktrees + lightweight checkpoints replace snapshots/rollback**: the
  compose-prompt aggregator, `kumo rollback` branch-switch snapshots and
  declarative `layout export/apply` were dropped in 0.7.0.
- **Scripts use `--json` + `KUMO_SOCKET_PATH`**, not the bincode wire; a fuller
  JSON/MCP surface comes after 1.0.
- **Product boundary**: kumo is a fast, terminal-native ADE (isolated
  worktrees, agents, reliable lifecycle state, one Inbox, checkpoints). It
  does not become an embedded editor, browser or project-management suite;
  broader work lifecycle (task sources, review, ship) belongs in 1.x or
  plugins.

## 🧊 0.8.0 — Contracts, persistence & freeze (RC)

The last release before 1.0 and its release candidate. It freezes what kumo
already ships as stable contracts, fixes the bugs that freezing exposed, and
makes sessions survive a reboot, so 1.0.0 is a version bump on a proven
build. The test for every item: would skipping it force a major bump or lose
user state later? Hardening that fails that test ships in 1.x patch and minor
releases, driven by real reports.

**P0 — the 1.0 gates:**

- ✅ **Additive-compatible wire protocol** (#27): a major/minor split
  (`PROTOCOL_VERSION` / `PROTOCOL_MINOR`); daemons answer unknown commands
  with `unsupported` and clients ignore unknown events.
- ✅ **Readiness-gated chained launch** (#28): `worktree create --agent` replies
  through the same server-owned, occupant-pinned wait as `agent start`, with
  deterministic success, `agent_not_ready`, `agent_replaced` and `timeout`.
- ✅ **Frozen control CLI** (#30): golden tests pin every `--json` reply shape,
  the error codes and the exit codes (`app/kumo/tests/cli_json.rs`),
  documented as the machine API. Fixes the success-shaped errors it exposed
  (`session kill <missing>`, `pane focus <missing>`).
- ✅ **Keymap stability** (#29): conflicting spellings of one chord resolve
  deterministically and warn; invalid bindings and leader warn in the TUI
  instead of being dropped silently.
- ✅ **0.x → 1.0 upgrade path** (#31): a genuine 0.7.6 `resume.json` fixture is
  loaded and its snapshots decoded on every test run, plus an opt-in live
  upgrade test from a released binary
  (`KUMO_PREV_BIN=… cargo test -p kumo --test upgrade -- --ignored`), run
  before each release.
- ✅ **Reboot recovery** (#32): the daemon autosaves `state.json` (layout within
  seconds, screen contents at most every `[persistence] autosave-interval`,
  final save on SIGTERM) and a cold start restores sessions, cwd, screens,
  aliases, and agents via their manifest `resume` command. Covers the
  `kill -9` restore check of gate 3.
- ✅ **Compatibility policy** (`docs/compatibility`): what semver covers per
  surface and the deprecation window.
- ✅ **Performance baseline**: typing latency, attach latency and memory per
  pane are measured by ignored benchmarks in `app/kumo/tests/runtime.rs` and
  documented, with how to reproduce them, in
  [`docs/performance`](docs/content/docs/performance.mdx).

**P1 — release:**

- ✅ Live upgrade test against the last 0.x release (0.7.6) passes.

**Moved to 1.x** (no contract depends on them; each ships when a report or a
measured need asks for it): `kumo config check` / `kumo doctor`, CI-tracked
benchmarks, `cargo fuzz` targets, the terminal compatibility audit
(vttest/esctest), the scripted multi-agent soak, socket trust-model tests,
shell completions and a man page, sessions in the command palette, the
Homebrew tap (prebuilt binaries already ship through the cargo-dist
installer; a tap needs a `homebrew-tap` repository and a `HOMEBREW_TAP_TOKEN`
secret before `installers` can include `"homebrew"`), and the
configurable scrollback limit (`[terminal] scrollback-limit`, today
hard-coded `10_000` in `app/kumo/src/daemon/pane.rs`).

## 🎉 1.0.0

Full customization + solid persistence, meeting the gate criteria above — the
**TOML** config schema is the frozen v1, macOS + Linux are first-class, Windows
stays experimental. The layout model — **sessions → tabs → panes** — is
recorded under [Design decisions](#-design-decisions), so it never
resurfaces as a perpetual issue. `0.8.0` is the RC that freezes the contracts;
1.0.0 ships the same build with no new features.

---

## 🧩 After 1.0 (1.x)

1.x releases are additive within the compatibility policy: new features never
break the frozen config, CLI JSON, protocol, or state contracts. Plugins wait
for 2.0.0.

**Windows parity** — never a 1.0 gate: full parity, stable
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
  The strongest long-term demand signal among comparable ADEs: saved SSH
  machines whose agents appear in the local AGENTS tab and Inbox, plus a
  phone/web client. Keep protocol changes before 1.0 compatible with a
  machine-scoped session id so this does not need a breaking bump.
- **Jujutsu workspaces**: create and clean up `jj workspace` checkouts
  alongside git worktrees, behind the same `worktree` surface.
- **Multi-repo tasks**: one task spanning several repos, each with its own
  worktree, grouped as one session with a combined changed-files view.
- **Agent session search** and **per-worktree dev-server ports**: search past
  agent conversations across sessions, and show the ports each worktree's
  processes listen on in its details view.
- **Deeper AI**: with 0.7.0's orchestration + isolated worktrees landed, the remaining stretch is cross-pane context — hand one pane's diff/checkpoint to another pane's prompt — plus multi-agent session hygiene (restart an agent with its native resume id).

**Agent continuity** (moved from 0.8.0 — the theme is *agents survive
anything*: a reboot or daemon crash brings every agent back into its own
conversation, not just a fresh shell):

- **Native agent resume** (first slice of the semantic integration contract
  below): an agent reports its native session identity over the socket — via
  an optional hook, or read from the screen where the agent prints it — and
  kumo persists it per pane in `state.json`. Each `agent-detection/<agent>.toml`
  declares how to resume (e.g. `resume = ["claude", "--resume", "{session}"]`).
  On a fresh daemon start, panes whose agent exited are relaunched through
  that command, one at a time with a configurable spacing, so a reboot does
  not stampede N agents at once. Agents without a resume rule keep today's
  behaviour (shell restored in the saved cwd).
- **Agent launch profiles**: named `[agents.<name>]` tables in `config.toml`
  (`kind`, `command`, `args`, `env`) usable from `agent start --profile`,
  `worktree create --agent`, and the TUI pickers. Covers custom presets,
  several accounts of one agent (`env = { CLAUDE_CONFIG_DIR = "…" }`), and
  per-profile permission flags without a dedicated setting for each.
- **Configurable worktree location**: `[worktree] directory` with `{parent}`,
  `{repo}`, and `{branch}` placeholders (default `{parent}/{repo}-{branch}`,
  today's hard-coded `worktrees.rs: worktree_path`), so worktrees can live in
  a dedicated folder instead of beside the repo.
- **Logical agent input**: `kumo agent send-keys <agent> <key>...` validates the
  complete key sequence before writing anything and handles `enter`, `esc`,
  arrows, and modifier chords. Keep `agent prompt` for text submission and
  reserve `send-keys` for deliberate interaction with approval/question UIs.
- **Semantic agent integration contract** (remainder): integrations also
  report lifecycle state. A pane has one explicit status authority at a time;
  when a healthy integration is active, screen rules do not compete with it.
  Screen manifests remain the zero-setup fallback. This avoids classifier
  drift.
- **Reliable long transcript reads**: add `agent read --source recent-unwrapped
  --lines N`, collecting alternate-screen history only while the agent is idle
  and restoring its viewport afterwards. `visible`, `detection`, output waits,
  and reads of working/blocked agents stay passive.
- **Agent-detection manifest refresh** (deferred from 0.7.0): `kumo reload`/`kumo update` refresh bundled `agent-detection/<agent>.toml` rules, with an **opt-in** background remote manifest check (default off — rules stay local-first and versioned).
- **Delegate Task handoff** (was 0.7.1): keep the user task separate from the
  checkpoint note, submit the task prompt only after the selected agent is
  ready, and refresh daemon/TUI state from the reply. Ship with a full E2E:
  `create --ai --agent → agent ready → task submitted → blocked/done →
  checkpoint set/current/list → remove`, including startup timeout/replacement,
  branch/path collisions, dirty or unmerged removal, daemon restart, shared
  dirs, and `.worktreeinclude`.
- **Structured command boundaries** (deferred from 0.7.0): OSC 133 prompt/command
  marks, structured tracebacks, the OSC 7/133 shell snippet installer, and the
  `leader+r` verify loop on top of `pane wait-output`.

**Quality of life** (moved from 0.9.0):

- **Floating popup panes**: a session-modal overlay pane (cell or percentage
  size) for one-off commands — lazygit, a scratch shell, —
  bindable as `{ popup = ["lazygit"] }`,
  without touching the tab's split tree. Closes with the process.
- **Worktree review loop** (separate from checkpoints): a terminal-native
  changed-files/diff view, comments anchored to file + line/hunk, unresolved /
  resolved state, and **Send review notes to agent** that batches every open
  comment into one contextual prompt. Start read-only plus annotations; staging,
  commit/push, AI attribution, and an embedded code editor are later layers.
- **tmux control-mode compatibility** so existing tooling (neovim, scripts) keeps working.
- **Asciinema export**: record a pane's session to a file / stream. Subsumes
  tmux-style **pipe-pane** (cut from 0.6.0): capturing a pane's output to a
  file lands as this richer export plus a pane-output stream scripts can
  consume over the socket, not as a separate daemon tap.

- **Smaller 0.7.0 deferrals**, each revisited on demand: a TUI broadcast
  prompt popup (`leader+B`) over `kumo agent broadcast`, Jira as a worktree
  `--from` source, and parent-worktree / sparse-checkout options in the
  worktree picker.

Any other quality-of-life ideas that round out the editor feel stay as 1.x polish.

## 🔌 2.0.0 — Plugins

Plugins are a new public API surface (manifest schema, plugin-API generation,
injected environment, event catalog, lockfile), so they ship as a major
release on top of the frozen 1.x contracts instead of widening 1.0.

- **Plugin system**: shareable workflow packages — a directory with a `kumo-plugin.toml` manifest the host validates, then launches its argv commands (bash/js/lua/rust/binary — whatever `argv` can run). The plugin owns language, deps, and files; kumo owns install, manifest validation, keybindings, panes, events, invocation context, and socket access. No SDK or WASM runtime: the **entire `kumo` CLI is the plugin API** (`KUMO_BIN_PATH` + the socket), so anything you can run as `kumo ...` a plugin can run. Keeps the featherweight story — same reason we called it "thin" (vs. zellij's runtime).
  - **Manifest** (`kumo-plugin.toml`): `[plugin]` header — `id`/`name`/`version`/`api` (plugin-API **generation, int**, host refuses unknown ones)/`kumo-min`/`platforms`/`tags` —, optional `[build] steps` (argv, run once on `add`, never on `dev`), and **one unified `[[entry]]` table** whose `kind` selects the surface: `command` (palette/menu/keys/`run`), `trigger` (`on` daemon events), `boot` (one-shot after resume + socket ready), `pane` (`place` = `tab`/`split`), and link interception is a `command` entry with a `link` regex. Entry ids qualify as `<plugin-id>.<entry-id>`; validated on `add`/`dev` (id charset, uniqueness, `api`/platform gating, regex compile), and `dev` links warn when top-level `platforms` is missing.
  - **Distribution**: `kumo plugin add owner/repo[/subdir]` — **prefers GitHub Release tarballs by semver tag, falls back to git clone**; live preview + confirmation in interactive terminals (`--yes`/`--ref` for CI); refuses to install over a dev link. `dev`/`undev` for local checkouts (no build run), `rm`/`ls`/`run`/`logs`/`where`/`upgrade` (semver-aware, `--all`, re-add to refresh pinned tarball)/`search`/`check`. A committed `plugins.lockfile` pins source/ref/version/checksum (`--frozen` for CI); registry state (managed checkout vs dev link) lives under `data_dir()`. Marketplace auto-indexed from the `kumo-plugin` GitHub topic — no submission form.
  - **Runtime**: host injects `KUMO_BIN_PATH`, `KUMO_SOCKET_PATH`, `KUMO_PLUGIN_ID/ROOT/CONFIG_DIR/STATE_DIR` (state dir is **host-managed** — durable state is a first-class contract, not an afterthought) and `KUMO_INVOCATION_JSON` (source, ids, clicked_url, …); plugins call back over the CLI or the raw socket; plain argv — no shell expansion. Events: `agent.working|blocked|idle|done`, `pane.created|closed|focused`, `session.created|removed`, `tab.created|removed`, `worktree.created`, `config.reloaded`, `boot` — fired by an async daemon-side dispatcher at the state-transition sites; failures are logged, never fatal.
  - **Keys & links**: plugins never declare chords — **users** wire plugin entries in their own `[keymap.bindings]` (`l = { plugin = "…", entry = "…" }`), visible in `leader+?`; no chord squatting. Link handlers intercept modified clicks before `open_url`, in manifest order.
  - **Marketplace site**: minimal static site — landing, `/docs/plugins/` authoring guide (trust model + event catalog), `/plugins` grid whose cards show **version/platforms/tags** (the index **parses manifests**, not just repo metadata) plus stars/language/last-push; a scheduled Action every 30 min indexes public repos tagged `topic:kumo-plugin` (no forks/archived) into a committed `index.json`; `kumo plugin search` reads the same index. Source resolution sits behind a trait from day one so a hosted/Docker-Hub-style registry can bolt on without breaking v1. Seed a `kumo-plugin-examples` cookbook. Trust model unchanged: plugins run as your user — install from sources you trust, preview and skim manifests first.
  - **Build order (phases)**: (0) manifest + validation + registry/lockfile + `add/rm/ls/dev/undev/check` → (1) protocol variants + async runner + env injection + `run/logs/where` + surface in context menu/palette → (2) trigger dispatcher at event sites + `boot` → (3) keybinding side-table + link interception → (4) `pane` entries (`tab`/`split`) → (5) site + indexer + `search` + examples repo. Each phase lands with `cargo test` + `cargo clippy` and an end-to-end `add → run` test against a fixture git repo.
- **Plugin actions in the command palette**, plugin entries as a **floating
  popup** placement, and **declarative sidebar view queries** read from
  plugins (deferred from 0.7.0).

---

Until then, kumo stays opinionated: it picks good defaults for you so it just
works, and you can follow the roadmap above as the knobs land. 🔧
