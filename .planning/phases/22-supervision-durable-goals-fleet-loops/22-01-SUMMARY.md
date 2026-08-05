---
phase: 22-supervision-durable-goals-fleet-loops
plan: "01"
subsystem: durable-goals
tags: [journal, schema-compatibility, goal-kernel, terminal-taxonomy]
requires: []
provides:
  - the measured cross-binary journal compatibility verdict (COMPATIBLE-AT-V5)
  - the authorized durable-record shape (additive at v5, no bump)
  - the single canonical Goal terminal taxonomy in wcore-types
  - a retained real-binary journal corpus for the F12 non-regression canary
affects:
  - crates/wcore-types
tech-stack:
  added: []
  patterns:
    - "output-only effective envelope: untrusted input deserializes into a request shape and must pass a resolver"
    - "structural anti-forgery: the evidence type is not Deserialize, so JSON has no route to the verified state"
key-files:
  created:
    - crates/wcore-types/src/goal.rs
    - .planning/phases/22-supervision-durable-goals-fleet-loops/22-01-JOURNAL-COMPAT.md
    - .planning/phases/22-supervision-durable-goals-fleet-loops/22-01-EVIDENCE/
  modified:
    - crates/wcore-types/src/lib.rs
decisions:
  - "Goal/Task/Wait records enter additively at SESSION_JOURNAL_SCHEMA_VERSION = 5 with no version bump (4-of-4 panel, basis majority)"
  - "The canonical Goal terminal taxonomy LIFTS Anvil's existing ten-variant enum rather than inventing a sixth vocabulary"
metrics:
  duration: one session
  completed: 2026-07-26
status: partial
---

# Phase 22 Plan 01: Durable Goal Kernel and Journal Compatibility — Summary

The F12 session journal was measured, cross-binary and single-variable, to admit
additive Goal/Task/Wait records at v5 without changing what an existing journal
replays to; a unanimous four-way panel authorized that shape; and the one
canonical terminal taxonomy was lifted from Anvil into `wcore-types` and shipped
green. **The durable kernel itself was NOT built.**

## Termination state

**Not one of the plan's four.** The plan enumerated: (1) complete, (2) migration
authorized and shipped, (3) escalated on the determination, (4) escalated on
scope. None applies. The determination succeeded and authorized the cheap option,
so states 2, 3 and 4 are out; state 1 requires the kernel, the stored-corpus
canary and both platforms green, and those were not reached before the session's
budget ran out.

The honest label is **PARTIAL — Tasks 1 and 2 complete on Linux with the Windows
leg of Task 1 open; Task 3 partially delivered (vocabulary and taxonomy only, no
kernel, no journal records, no reducer change).**

## What was done

**Task 1 — the determination.** A real 84,327-byte journal was produced by the
shipped release binary at `2ecdfdf5` on `hetzner-dsm` from a real product
invocation, carrying 9 distinct durable event types. Five single-variable
measurements were taken with two differently-built binaries touching that one
file. M1 gave a byte-identical SHA-256 over the reduced state; M2 showed an
appended record moves nothing but the chain head; M3 showed the old binary fails
CLOSED on a version-skewed journal (exit 3, zero bytes, explicit unknown-variant
error) rather than truncating silently; M4 showed a pre-change snapshot and
authority binding are accepted unchanged; M5 showed the writer lease is released
on drop. A sixth, unplanned observation (M0) is worth keeping: the reducer's
`apply_event` match is exhaustive with no wildcard, so a new durable record
cannot enter the enum without a deliberate reduction arm.

**Task 2 — the decision.** Both required measurements were taken rather than
asserted: 14 resolvable `SOURCE-CITE:` lines re-read from the tree (the
planning-time count of 49 frozen legacy event types was re-measured and agreed),
and a live scratch-checkout probe on `hetzner-dsm` that bumped the schema
constant alone and printed
`MIGRATION-COST-PROBE: failing_tests=3 total_tests=76 commit=2ecdfdf5...`.
All four panel members — codex gpt-5.6-sol, gemini 3.1-pro-preview, kimi K3, and
an internal adversarial pass that argued against its own prior — returned
`additive-at-current-version` and `VERDICT_SOUND=yes`. Committed on basis
`majority`. Dissent preserved naming all three options.

**Task 3 — partial.** `crates/wcore-types/src/goal.rs` lands the Goal vocabulary
and the single canonical terminal taxonomy, with 7 tests green on Linux
(`cargo test -p wcore-types --lib goal`: 7 passed), clippy clean with `-D
warnings`, fmt clean. The verified state is structurally unreachable from
model-authored content because `HostGateObservation` is not `Deserialize`, and
the negative case is the load-bearing test.

## What was NOT done, stated plainly

- **No durable kernel.** `crates/wcore-agent/src/goal/kernel.rs` does not exist.
  No `SessionEvent` variants were added, no reducer arm, no `ReducedSessionState`
  field, no cursor exposure. F22-02 is therefore **NOT complete**.
- **No stored-corpus regression canary.** The corpus is retained as evidence but
  no test pins its reduction. Behavior 6 — the F12 non-regression canary, which
  the plan called one of the two that matter most — is unbuilt.
- **No crash-matrix completeness test** over Goal transitions (behavior 7).
- **Windows M1–M5 were never taken.** The Windows leg produced a real
  81,093-byte WJ01 journal from the real Windows binary at the same commit, and
  the writer lock file was observed present — but the reduce instrument needed
  for the cross-binary comparison was still compiling `wcore-agent` on a
  contended shared box when the budget ran out. Threat T-22-06 (Windows
  byte-range lock semantics under the `#[cfg(unix)]`-gated lease) is **open**.
- **No tool frame in either corpus.** The provisioned Anthropic credential on
  `hetzner-dsm` returns HTTP 401; a working credential is reserved to Sean. Both
  legs' `product-stdout.txt` are zero bytes, so two of this task's own gate
  clauses are RED and are reported as such rather than worked around.

## Deviations from plan

**[Rule 3 — blocking] The plan directory was not in the worktree.** The
orchestrator's worktree was created from `waylandcore` at the v0.12.25 release
commit `61b79c4f`, which has no `.planning/`. The plans live in
`waylandcore-ferrox` on `plan/f20-unified-audit-repair`. Since `61b79c4f` is an
ancestor of that branch, the branch was fetched into a private ref and the
worktree fast-forwarded to `2ecdfdf5`. Every sibling phase in this wave will hit
the same thing. Recorded because it is an orchestration defect, not a plan defect.

**[executor error, recorded not hidden] The first measurement run compared two
different journals.** An unrelated `wayland-core providers` invocation had
created a second session, and the measurement script picked it by glob while the
baseline had been taken from the first. M1/M2/M4 were re-taken with the journal
pinned explicitly. The first run's M1_RESULT=DIFFERENT was a script defect, not a
product observation.

**[executor error, recorded not hidden] The migration probe's first verdict line
was self-passing.** It counted `^ *FAIL [` and nextest prints a retried failure
as `TRY 2 FAIL [`, so it printed `failing_tests=0` while the summary said 3. The
number was re-derived from nextest's own summary line. This is exactly the shape
`lint-plan-gates.py` exists to catch, produced by this executor.

## Requirements

- **F22-06** — evidence gathered, verdict recorded, decision authorized. **NOT
  complete**: the plan makes completion conditional on both platforms green and
  the stored corpus replaying identically, and neither holds.
- **F22-02** — **NOT complete**. The vocabulary and taxonomy exist; the durable
  kernel does not.

## Phase 21 seam dependency

None was consumed and none was changed. `resolve_goal_authority` deliberately
records what an envelope WAS rather than inventing a second intersection
primitive, so no Phase 21 seam is redefined (termination state 4 was not entered).
The Phase 21 artifacts were not read at execution time — recorded as a gap.
