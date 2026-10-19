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
