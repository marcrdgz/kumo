# Kumo for AI Agents

Kumo is a terminal multiplexer where the daemon owns your worktrees, panes, and agents. Every worktree is an isolated git checkout — perfect for running 5 agents in parallel without stepping on each other's files.

This file is auto-loaded in every AI pane so you orchestrate natively instead of sending keystrokes.

## Worktree checkpoints — KEEP FRESH AUTOMATICALLY (MANDATORY)

Every worktree carries a lightweight **comment** (free-text) + **status** (`todo` / `in-progress` / `in-review` / `completed`). You **must** keep it fresh without being asked — humans and other agents see it instantly in the TUI, `kumo worktree list --json`, and the picker. This is **automatic, not optional**.

**Statuses:** `todo` · `in-progress` · `in-review` · `completed` (aliases: `wip`→`in-progress`, `review`→`in-review`, `done`→`completed`). Use `—` or empty to clear.

**Pattern (do this automatically, every time):**
```sh
# 1. Read before you clobber — preserves human context
kumo worktree current --json
# or: kumo worktree current --json | jq .comment

# 2. Write as you make progress (run this without asking)
kumo worktree set --comment "reproduced auth failure; testing credential-chain fix (src/auth.ts)" --status in-progress --json
kumo worktree set --comment "fix implemented; running integration tests" --status in-review --json
kumo worktree set --comment "verified — ready for review, goal: reduce API calls per #298" --status completed --json
```

**Aliases:**
```sh
kumo worktree set --worktree active --comment "..." --workspace-status in-progress --json  # same as --status
kumo worktree current --worktree active --json  # same as --path active
```

**Good moments to update:**
- Finished a meaningful slice
- Confirmed or refuted a hypothesis
- Hit a blocker (waiting on input, upstream bug, missing access)
- Transitioning phase (investigation → fix → verify)

**Format:** First line is the action — what just happened, where, and next step. Keep it action-oriented. Add a second line for goal/context if needed.

**On creation you can seed it:**
```sh
kumo worktree create --ai "my task" --note "starting on the login flow" --json
```

Seeded via the TUI's **Advanced → Note** field too.

## Orchestrating Kumo (beyond checkpoints)

You are inside a pane but you can drive the whole workspace via the daemon socket (already injected as `KUMO_SOCKET_PATH` and `KUMO_BIN_PATH` — no config needed; disable with `[worktree] expose-socket = false`).

**Sessions / tabs / panes:**
```sh
kumo session list --json
kumo pane list --json
kumo pane send-keys -p 123 "echo hi"
kumo pane wait-output 123 --regex "✔ done" --timeout 30s --json
```

**Agents:**
```sh
kumo agent status --json                 # all agents + pane ids
kumo agent wait 123 --until blocked --timeout 60s
kumo agent prompt 123 "continue" --wait done
kumo agent read 123 --source visible|recent|detection|traceback --json
kumo agent start --kind opencode --pane 123 -- --flag
kumo agent broadcast "fix lint" --filter working
```

**Worktrees:**
```sh
kumo worktree list --json                # includes comment/status/is_ephemeral
kumo worktree create --ai --branch feat/x --from main --note "..." --agent claude --json
kumo worktree rm /path/to/wt --force
```

**Inbox:** `leader+i` (TUI) jumps to blocked/done agents. Filter via CLI.

## Rules

- Read `kumo worktree current --json` before you overwrite a comment — preserve any human-written goal.
- Keep the first line action-oriented; don't dump full logs into the comment (use `agent read` for that).
- Checkpoints survive `kumo update --resume` and daemon restarts (atomic `worktrees.json`).

That's it — keep the checkpoint fresh and the humans will always know where you are without asking.
