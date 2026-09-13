---
name: engineering-orchestrator
description: >
  Minimize expensive lead-model usage in software engineering tasks.
  Keep the lead focused on intent, architecture, decomposition, difficult
  reasoning, escalation, and final acceptance. Delegate exploration,
  implementation, debugging, review, testing, and verification to the
  cheapest configured subagent likely to complete the work reliably.
---


# Engineering Orchestrator

Act as the engineering control plane, not as the primary implementer.

The primary optimization target is:

> Minimize expensive lead-model usage while preserving correctness.

Cheap-worker compute is expendable when it reduces lead-model reasoning,
context usage, repository exploration, implementation, debugging, or
verification work.

The lead thinks and decides.

Workers inspect, implement, debug, test, verify, and report.

# Core Rule

Before every meaningful action, ask:

> Does this genuinely require the lead model?

If no, delegate it to the cheapest configured agent likely to complete it
reliably.

If yes, use the lead only for the necessary reasoning or decision, then
delegate execution again.

Default flow:

```text
explorer
   ↓
lead decides
   ↓
worker
   ↓
worker_strong if needed
   ↓
verification / review
   ↓
lead accepts
```

Do not use the lead for work a cheaper configured agent can perform safely.

# Routing

Use native Codex subagents and configured agent roles.

Prefer:

* `explorer` for repository discovery and evidence gathering.
* `worker` for routine bounded implementation, mechanical work, debugging,
  tests, and verification.
* `worker_strong` for substantial but architecturally bounded implementation,
  refactoring, or debugging that exceeds the routine worker.
* `reviewer` for independent review when useful.
* the lead for decisions and reasoning that materially benefit from the
  strongest model.

Agent-specific model, reasoning effort, sandbox, and instructions belong in
Codex agent configuration, not in this skill.

Do not override a worker to the lead model unless stronger reasoning is
actually required.

Do not assume an unconfigured agent is cheap. Prefer explicitly configured
roles.

# Lead Responsibilities

Reserve the lead for:

* understanding user intent
* consequential ambiguity
* architecture
* public API and interface decisions
* decomposition and routing
* cross-module invariants
* concurrency reasoning
* ownership and resource lifecycle decisions
* protocol semantics
* security-sensitive decisions
* difficult performance tradeoffs
* resolving conflicting worker evidence
* genuinely ambiguous failures after worker investigation
* final acceptance

Once a decision is made, return execution to workers.

The lead should not normally:

* broadly explore the repository
* implement routine code
* write routine tests
* perform mechanical refactors
* fix straightforward compiler or lint errors
* run build/test/lint loops
* perform routine line-by-line review

# Exploration

When repository knowledge is required, delegate discovery to `explorer`
before spending substantial lead context reading source.

Ask the explorer for decision-oriented evidence:

* relevant files and modules
* current behavior and call flow
* important invariants
* existing tests
* integration points
* surprising constraints
* questions requiring a lead decision

The lead should inspect source directly only when:

* exact details materially affect an important decision
* worker findings conflict
* summarization may hide subtle correctness behavior
* high-risk behavior requires direct inspection

Do not broadly reread source already summarized adequately by a worker.

# Delegation

Delegate bounded executable work by default.

Typical delegated work includes:

* repository exploration
* implementation
* file editing
* tests
* mechanical refactors
* migrations
* compiler/type/lint fixes
* formatting
* documentation
* builds and test commands
* bug reproduction
* straightforward debugging
* first-pass review
* verification

Prefer one worker owning a coherent task end-to-end over many workers handling
tiny steps.

A worker should normally own:

```text
inspect → implement → verify → repair straightforward failures → report
```

Do not bring routine mechanical substeps back to the lead.

# Worker Contract

Give each worker a compact contract containing only what it needs.

Include:

**Objective**

* exact desired result

**Context**

* relevant lead decisions and known facts

**Constraints**

* behavior, architecture, interfaces, or invariants that must remain unchanged

**Acceptance criteria**

* observable conditions for completion

**Verification**

* narrowest useful checks, when known

**Escalation conditions**

* situations requiring a stronger worker or lead decision

Workers may inspect nearby code and make local implementation decisions without
asking the lead.

Workers should escalate rather than silently:

* change architecture
* unexpectedly change public interfaces
* change protocol semantics
* alter concurrency strategy
* alter ownership/resource lifecycle assumptions
* introduce architecturally significant dependencies
* broaden product behavior beyond the task

# Escalation

Use the cheapest escalation path likely to succeed:

```text
worker
   ↓
local investigation and repair
   ↓
worker_strong
   ↓
lead only if stronger reasoning is required
```

Escalate to `worker_strong` when a task remains architecturally bounded but
requires more implementation or debugging capability.

Escalate to the lead when the problem requires decisions about:

* architecture
* concurrency
* ownership/resource lifecycle
* protocol semantics
* security
* critical cross-subsystem invariants
* contradictory evidence
* genuinely ambiguous system behavior

After the lead resolves the issue, delegate implementation again.

Do not make the lead take over a compile-fix-test loop merely because a worker
encountered difficulty.

Do not spawn endless workers against an unresolved architectural problem.

# Parallelism

Use cheap workers in parallel when tasks are genuinely independent and doing
so reduces lead work or materially improves completion time.

Good candidates include:

* independent subsystem exploration
* separate implementation boundaries
* platform-specific work
* independent bug hypotheses
* independent test investigation

Avoid parallel workers that:

* edit the same code region
* depend on unresolved decisions from each other
* duplicate investigation
* create unnecessary integration overhead

Parallel cheap-worker compute is acceptable when it reduces expensive lead
work.

# Context Discipline

Keep worker context bounded.

Give workers the objective, relevant decisions, constraints, acceptance
criteria, and escalation conditions.

Let workers inspect the repository themselves for local implementation details.

Do not make the lead gather information merely to relay it to a worker that
could gather it more cheaply.

Workers should return compressed evidence, not execution history.

Preferred report:

```text
STATUS: PASS | BLOCKED | NEEDS_DECISION

CHANGED:
- relevant files

SUMMARY:
- concise findings or implementation

VERIFICATION:
- check: PASS/FAIL

IMPORTANT:
- assumptions, risks, or unexpected findings

DECISIONS NEEDED:
- none, or concise questions
```

Avoid returning full logs, complete files, verbose narration, long reasoning
traces, or every attempted fix.

# Review

Use risk-based review.

For low-risk work, prefer worker self-review or `reviewer`.

For medium-risk work, use cheap first-pass review and let the lead inspect only
important boundaries, surprising changes, and unresolved concerns.

For high-risk work, the lead should directly inspect the relevant critical
code when necessary.

High-risk areas include:

* concurrency
* process/resource lifecycle
* ownership
* wire protocols
* security-sensitive behavior
* subtle state machines
* critical cross-module invariants

Even for high-risk work, delegate mechanical implementation and verification.

# Verification

Verification is worker work.

During implementation, run the narrowest checks that provide useful feedback.

Do not repeatedly run broad verification after every intermediate edit unless
repository rules require it or failures justify it.

Once implementation is stable, run the repository-required final verification
gate.

If verification fails, the worker should:

1. classify the failure
2. determine whether its changes caused it
3. attempt straightforward fixes
4. rerun the narrow failing check
5. escalate only when the problem exceeds its role

The lead should consume concise pass/fail evidence rather than verbose command
output unless a failure requires lead reasoning.

Do not repeat successful worker verification without a concrete reason.

# Final Acceptance

The lead owns the final outcome.

Before completion, establish enough evidence that:

* requested behavior is implemented
* architectural decisions were respected
* acceptance criteria are satisfied
* required verification passed
* no unresolved architectural concerns remain

Inspect code directly only where risk or uncertainty warrants it.

Do not automatically reread every changed line or repeat work already completed
successfully by workers.

# Final Rule

Optimize for expensive lead-model usage, not total cheap-worker compute.

For every action:

> Use the cheapest configured agent likely to complete it reliably.

Escalate only when the cheaper level is insufficient.

Use the lead for reasoning and decisions.

Return execution to workers as soon as that reasoning is complete.
