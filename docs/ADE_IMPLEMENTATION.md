# ADE implementation tracker

Kumo’s ADE direction is a persistent terminal environment for running,
observing, and reviewing agent work. The product combines the reliability of
the Herdr runtime with the workspace and review flow of Orca. This tracker is
the implementation view; it does not imply feature parity or completion.

## Delivery map

| Phase | Branch | Base | Status | Outcome |
| --- | --- | --- | --- | --- |
| 1. Runtime | `feat/ade-01-runtime` | `main` | Delivered in `086c12f` | Keep live agent terminals reliable across viewers, shutdown, and daemon restart. |
| 2. Workspaces | `feat/ade-02-workspaces` | phase 1 | In progress | Persist workspace/run identities and expose a durable ADE inbox through daemon, CLI, and TUI. |
| 3. Orca compose, context, and review | `feat/ade-03-compose-review` | phase 2 | Planned | Orca Draft → Context → Review: compose Drafts, preview Context, and capture Reviews. |
| 4. Parallel runs | `feat/ade-04-parallel` | phase 3 | Planned | Run isolated work per recipient and expose each outcome clearly. |
| 5. Integrations | `feat/ade-05-integrations` | dependency TBD | Pending priority | Choose the next integration surface: SSH, GitHub issues, or a richer TUI review flow. |

## Phase 1 — runtime reliability (delivered)

Commit `086c12f` hardens the daemon’s runtime behavior:

- bounded PTY and client command queues prevent unbounded backlog;
- daemon processing is bounded per loop so commands, PTY output, and clients
  continue to receive service under load;
- all viewers diff from the same frame generation, so a second viewer cannot
  consume the first viewer’s baseline;
- restart preserves live PTY masters and child ownership, allowing panes and
  agents to survive an in-place daemon exec;
- inherited children are polled and reaped safely, avoiding PID reuse hazards;
- shutdown starts pane cleanup together and joins cleanup workers;
- runtime integration tests cover incremental output to two viewers and a real
  exec restart followed by child exit.

Remaining runtime work is explicit before this phase can support orchestration:

- audit and remove blocking Git and PTY input paths from the daemon’s command
  loop, with cancellation and bounded waits where an operation can stall;
- supervise all long-lived runtime threads, report worker failure, and define
  restart or shutdown behavior instead of silently losing a worker.

Acceptance criteria:

- two attached viewers receive the same incremental pane generations;
- a daemon restart preserves pane identity, screen state, scrollback, and live
  shell or agent IO;
- exited children are reaped exactly once and shutdown leaves no live pane
  workers;
- blocked Git or PTY operations cannot starve command dispatch;
- worker failures are observable and produce a defined recovery outcome.

## Phase 2 — workspace and run identities (in progress)

The persistence store, protocol snapshot, daemon lifecycle reconciliation,
`kumo ade list|focus|ack`, and the TUI durable inbox modal are implemented on
the phase 2 branch. The modal keeps selection by inbox ID, shows workspace and
run identity, supports focus and acknowledgement, and remains usable after a
restart when no live pane is attached. Drafts, context bundles, review records,
and full historical inbox browsing remain Phase 3 work.

Introduce durable workspace and run records that survive detach, restart, and
multiple viewers. A run identifies an agent execution independently of its
pane, while a workspace identifies the repository or worktree it operates in.
The inbox is a stable, ordered stream of actionable events (approval,
question, blocked state, completion, or review request), keyed by run identity
so events remain addressable after layout changes.

Acceptance criteria:

- every workspace and run has a stable persisted identifier and human label;
- runs can be found and focused after daemon restart or pane relocation;
- inbox entries are ordered, durable, deduplicated, and link back to the run
  and workspace;
- state changes are available to both the TUI and control CLI.

## Phase 3 — Orca compose, context, and review (planned)

Implements Orca's compose → context → review loop on top of the durable
Workspace, Run, and Inbox from phase 2. A **Draft** (Orca: the unsent
request) is composed before any Run exists. The Draft editor shows the
selected **Recipients** (Orca: each agent/workspace that will receive its
own isolated Run), the target **Workspace**, and a **Context Preview**
(Orca: the Context bundle assembled from terminal and repository state —
scrollback selection, recent output, diff, and worktree snapshot). A
**Review** (Orca: `Approved` · `Changes Requested` · `Rejected`) becomes a
first-class, durable record linked to the Draft and Run, and surfaced via
the Inbox and Run view.

Orca terminology in this phase: **Draft** = editable request;
**Recipients** = per-Run targets; **Context** / **Context Preview** =
attachable material with source attribution; **Run** = execution of a Draft
for one Recipient; **Review** = decision on a Run.

Acceptance criteria (Orca-aligned):

- Drafts can be edited, saved, reopened, and discarded without creating a
  Run (Draft lifecycle precedes Run);
- Context Preview enumerates its Orca Context sources and allows removing
  or revising each source before send;
- sending a Draft records the exact Draft revision and Context snapshot
  used to create each Run (one Run per Recipient);
- Reviews are durable, attributable, and visible from the Inbox and Run
  view with explicit `Approved` / `Changes Requested` / `Rejected` status.

## Phase 4 — isolated parallel runs (planned)

Support parallel execution per recipient with isolated workspaces or
worktrees. Each run receives its own lifecycle, terminal stream, and outcome;
one recipient’s failure, approval, or completion must not obscure the others.
The review surface aggregates outcomes while preserving per-recipient detail.

Acceptance criteria:

- one compose action can create multiple independently addressable runs;
- each run has an isolated working directory and branch or equivalent
  workspace boundary;
- output, status, errors, and review feedback are attributable to exactly one
  run;
- partial success is represented explicitly and can be reviewed or retried
  per recipient;
- daemon restart and viewer reattach preserve every active run.

## Phase 5 — integrations (priority pending)

The integration scope is intentionally undecided. The next branch depends on
product priority among SSH-based execution, GitHub issue intake and linking,
or a richer in-TUI review experience. Record the decision and its dependency
before implementing `feat/ade-05-integrations`.

Acceptance criteria after the priority is chosen:

- the selected integration has a documented boundary and failure behavior;
- external identifiers map back to stable workspace, run, and inbox records;
- unavailable credentials or network services leave local runs usable;
- the integration is covered by deterministic tests plus an end-to-end smoke
  path where the environment permits it.

## Verification gate

Every phase that changes Rust code must pass all workspace gates from the
repository root:

```text
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
```

Clippy must be clean. Documentation-only changes do not require a cargo run,
but the gates are mandatory for each implementation branch before review.
