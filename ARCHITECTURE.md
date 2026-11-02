# Kumo architecture

Kumo is a terminal multiplexer with a Rust TUI frontend built on **ratatui**,
using **libghostty-vt** (Ghostty's headless terminal emulator, vendored and
compiled with Zig) for terminal emulation and **portable-pty** for PTY
management. The daemon and the TUI client are one binary, `kumo`.

User-facing documentation lives in `docs/` (published at
<https://marcrdgz.github.io/kumo/docs>); the plan for upcoming releases is in
[ROADMAP.md](ROADMAP.md).

## Smart renderer, dumb viewport

The daemon is the **single source of truth** for everything it has open —
sessions, tabs, the **semantic layout tree** (splits in ratios, never pixels),
the PTYs, and per-pane terminal content — and it **never renders chrome**: no
borders, box-drawing characters, sidebar, or status bar ever enter the wire.
Clients are **dumb viewports**: they receive two things and draw everything
themselves (through `crates/kumo-protocol`):

- **Layout** (`DaemonEvent::Layout`): sessions → tabs → splits (with ratios) →
  panes (title, cwd, agent status). Clients compute geometry, request pane
  sizes via `PaneResize`, and draw their own borders and chrome.
- **Pane content** (`DaemonEvent::PaneFrame`): each pane's terminal grid
  (rendered by the daemon's Ghostty core), streamed as dirty-row cell patches.

Everything else is a **command** (`Command`), tmux/zellij style — the whole
multiplexer is drivable from the CLI, the TUI, the desktop app, or a script
(`kumo session|tab|pane|agent|worktree …`).

The daemon has no keyboard layer: the TUI client owns the leader keymap
(`app/kumo/src/cli/bindings.rs`) and translates keys into commands. Opening a
sidebar or resizing is a client concern that never mutates the daemon's state
beyond a `PaneResize` or a command.

## Wire protocol compatibility

`crates/kumo-protocol` is shared with the native desktop client
(`kumo-desktop`, a separate project pinned by git rev). An additive change only
**appends** a variant to `Command`/`DaemonEvent` and bumps `PROTOCOL_MINOR`;
anything else (new fields, reordering, nested enum variants) bumps
`PROTOCOL_VERSION`. The user-facing promises are in the compatibility policy
(`docs/content/docs/compatibility.mdx`).

## Persistence and in-place updates

- `kumo update` swaps the binary and the daemon re-execs itself as
  `kumo daemon --resume <file>`, inheriting the live PTYs from a transient
  resume file (`app/kumo/src/daemon/state.rs`).
- For reboots, the daemon keeps `state.json` in the state directory current
  (`app/kumo/src/daemon/app/autosave.rs`) and restores sessions, screens and
  agent conversations on a cold start.
- Released state formats are pinned by fixtures and upgrade tests
  (`app/kumo/tests/upgrade.rs`, `app/kumo/tests/reboot.rs`).

## Agents

Agent lifecycle detection (`working · blocked · idle · done · unknown`) runs in
the daemon from a `Snapshot` of each pane's terminal buffer
(`app/kumo/src/daemon/agents/`). The classifiers are data-driven: bundled TOML
manifests in `agents/rules/`, overridable from
`<config dir>/agent-detection/`. Server-owned waits (`agent wait`,
`agent prompt --wait`, `pane wait-output`) live in
`app/kumo/src/daemon/app/waits.rs`.

## Key source files

- **`app/kumo/src/main.rs`**: binary entry — `kumo daemon` | control CLI +
  update | TUI launch.
- **`app/kumo/src/daemon/app.rs`**: the engine — sessions, layout tree ops,
  PTYs, agents, themes.
- **`app/kumo/src/daemon/app/server.rs`**: headless daemon loop — command
  dispatch, per-client routing, layout + pane-frame streaming.
- **`app/kumo/src/daemon/app/commands.rs`**: the daemon's command handlers.
- **`app/kumo/src/daemon/app/ui.rs`**: per-pane content rendering (`tick`) and
  the semantic `layout()` export; no chrome.
- **`app/kumo/src/daemon/frames.rs`**: per-pane `Buffer` → `PaneFrame`
  serialization.
- **`app/kumo/src/daemon/pane.rs`**: `Pane` = PTY + `libghostty-vt` terminal.
- **`app/kumo/src/daemon/vt.rs`**: hand-written FFI to the `libghostty-vt` C
  API plus the safe `Terminal` wrapper.
- **`app/kumo/src/daemon/pty.rs`**: `portable-pty` wrapper.
- **`app/kumo/src/cli/client.rs`**: the TUI client connection and loop.
- **`app/kumo/src/cli/client_view.rs`**: client-side view — lays out from the
  semantic tree, maps the keymap to commands; rendering, mouse, copy-mode and
  the sidebar live in `cli/client_view/`.
- **`app/kumo/src/cli/cli.rs`**: the control CLI and its `--json` output.
- **`app/kumo/build.rs`**: compiles the vendored `libghostty-vt` Zig library.
- **`crates/kumo-core/src/config.rs`**: XDG directory resolution and config
  parsing.
- **`crates/kumo-core/src/`**: also layout, themes, the CLI self-updater
  (`update.rs`), the desktop updater (`updater.rs`) and worktrees.
- **`vendor/libghostty-vt/`**: vendored Ghostty terminal emulator (Zig + C).
