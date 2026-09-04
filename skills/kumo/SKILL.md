---
name: kumo
description: Kumo terminal multiplexer — worktrees, checkpoints, orchestration. Use when inside a Kumo pane/worktree or driving sessions/panes/agents; automatically keep worktree checkpoints fresh via kumo worktree set.
---

# Kumo — Agent Skill

Kumo is a terminal multiplexer where the daemon owns your worktrees, panes, and agents. Every worktree is an isolated git checkout.

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

**Good moments to update (do it then):**
- Finished a meaningful slice
- Confirmed or refuted a hypothesis
- Hit a blocker (waiting on input, upstream bug, missing access)
- Transitioning phase (investigation → fix → verify)

**Format:** First line is the action — what just happened, where, and next step. Keep it action-oriented. Add a second line for goal/context if needed.

**On creation you can seed it:**
```sh
kumo worktree create --ai "my task" --note "starting on the login flow" --json
```

**Rules:**
- Read `kumo worktree current --json` before you overwrite a comment — preserve any human-written goal.
- Keep the first line action-oriented; don't dump full logs into the comment (use `agent read` for that).
- Checkpoints survive `kumo update --resume` and daemon restarts (atomic `worktrees.json`).

## Full guide versioned with binary

This file is the full guide. For the version matching your installed `kumo` binary, you can also run:
1. Resolve CLI: `KUMO_BIN_PATH` if set, else `kumo`.
2. Run: `kumo skills get kumo --full` (or `--json` for automation).

Flags live in the binary so they never drift.
