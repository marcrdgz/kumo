# Render fixtures

Raw PTY output fed to `Pane` by the render tests in `src/daemon/pane.rs`.
Each `.raw` file is the exact byte stream a 120x40 terminal received.

| File | Source |
| --- | --- |
| `claude.raw` | Real: Claude Code 2.1.303 — startup, one `say hello` turn, `thanks` typed but not sent. The greeting's user name was replaced with `Alex`. |
| `codex.raw` | Real: codex-cli 0.160.1 (`--no-daemon`) — startup, update prompt skipped, `say hello` typed but not sent. |
| `opencode.raw` | Real: opencode 1.18.31 — startup, `say hello` typed but not sent. |
| `claude-edit-diff.raw` | Synthetic: approximates Claude's Edit diff preview, whose highlighted spans include blank cells (the shape behind `aa5c6fe`). |

Real captures were recorded with `capture.py`, which runs the CLI under a
120x40 PTY, answers the terminal queries TUIs block on, and replays scripted
keystrokes. Run it from an empty directory so no local paths leak in, e.g.:

```sh
python3 capture.py opencode.raw /tmp/kumo-fixture "w10;tsay hello;w2" -- opencode
```

Before committing a new capture, check it for user names, e-mail addresses
and home paths.

# Upgrade fixtures

`state/resume-<version>.json` is the resume file a released daemon wrote for
`kumo update`: the sessions, tabs, layout, agent aliases and ghostty snapshots
the next release must adopt. The tests in `src/daemon/state.rs` load every
fixture and decode its snapshots, so a schema or snapshot-format change that
would lose sessions on upgrade fails CI.

Capture one per release with the released binary (only the last 0.x release
needs to be kept once 1.0 ships):

```sh
sh state/capture.sh "$(command -v kumo)" state/resume-0.7.6.json
```

The 0.7.6 fixture was captured from the 0.7.6 release (its `echo` marker reads
`FIXTURE_SCROLLBACK_0_7_6`); paths are throwaway `mktemp` directories.
