---
name: kumo
description: Kumo terminal multiplexer — worktrees, checkpoints, orchestration
---

# Kumo — Agent Skill (stub)

This is a discovery stub. Do not invent flags from memory — the full guide is versioned with the binary.

**When to use:** you are inside a Kumo pane, worktree, or need to drive sessions/panes/agents from a script.

**How to load the full guide (version-matched):**
1. Resolve the CLI for this session: `KUMO_BIN_PATH` if set, else `kumo`.
2. Run: `kumo skills get kumo` (or `--full` for the long guide, `--json` for automation).
3. Prefer `--json` for scripts and do not invent flags.

**Quick checkpoint pattern (Orca-aligned):**
```sh
kumo worktree current --json          # read before you clobber
kumo worktree set --comment "reproduced auth failure; testing fix (src/auth.ts)" --status in-progress --json
# Orca aliases also work: --workspace-status for --status, --worktree active for the active worktree
```

Flags live in the binary so they never drift — always load via `kumo skills get`.
