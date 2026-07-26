---
phase: 22-supervision-durable-goals-fleet-loops
plan: "02"
subsystem: orchestration
tags: [census, terminal-taxonomy, loop-owner, criterion-3]
requires:
  - "22-01"
provides:
  - the five-strategy loop-owner census measured against the source
  - the finding that Anvil already carries a fit-for-purpose terminal taxonomy
affects: []
tech-stack:
  added: []
  patterns: []
key-files:
  created:
    - .planning/phases/22-supervision-durable-goals-fleet-loops/22-02-LOOP-OWNER-CENSUS.md
  modified: []
decisions:
  - "The canonical terminal taxonomy is a LIFT of Anvil's existing enum, not a new one — measured, and it changed the design"
metrics:
  duration: partial session
  completed: 2026-07-26
status: partial
---

# Phase 22 Plan 02: One Loop Owner, One Terminal Transition — Summary

**Task 1 of 4 complete. Tasks 2, 3 and 4 not executed.** The census is delivered
and it changed the phase's design; nothing was subordinated, no adapter surface
was built, and no engine was driven through the real binary.

## What was done — Task 1, the census

`22-02-LOOP-OWNER-CENSUS.md` measures all five engines against the tree at
`2ecdfdf5`: per strategy the terminal shapes it can actually produce, the retry
owner and its exact bound, the verification owner, whether it nests, and what a
naive canonical mapping would lose.

Three results are load-bearing for Success Criterion 3:

1. **Anvil already has the taxonomy.**
   `crates/wcore-agent/src/orchestration/anvil/mod.rs:52` defines a ten-variant
   `TerminalState` whose own comment says "the COMPLETE enum... There is no
   silent fourth exit". It already reserves `Verified` for a real Tier-1 gate and
   already keeps `CriteriaChecked` / `SelfChecked` / `NeedsEscalation` as
   explicit partially-checked categories. The plan's truths described five
   vocabularies needing to become one and did not mention that one of the five
   was already fit for purpose. That omission is the difference between "define a
   new taxonomy" and "lift the existing one", and it changed the design. The
   lifted taxonomy shipped in 22-01 as `wcore_types::goal::GoalTerminalState`.

2. **Council can never reach `verified`.** Its verification owner is an LLM
   aggregator. Under the F20-GATE-02 discipline Phase 22 inherits, a model judge
   cannot mint a verified terminal state. Same for ForgeFlows (validates output
   *shape*), Fleet (counts `succeeded` booleans) and Direct (produces no verdict).
   Only Anvil runs a real executable gate. That fact is now written down in
   exactly one place: `GoalStrategy::can_produce_host_observed_evidence()`.

3. **Fleet cannot be adapted by its return type.** The fleet-level result is a
   caller-chosen generic `T` produced by a `FleetReducer<T>`. The plan's truths
   said Fleet produces "`ShardSummary` or `FleetError`"; `ShardSummary` is the
   per-shard intermediate. An adapter written from the plan's sentence would bind
   to whatever the caller felt like returning. The adapter must bind at
   `ShardSummary`, before the reducer collapses it.

Also measured: `MAX_SCHEMA_RETRIES = 2` at runner.rs:416 (so `1 + 2 = 3`
dispatches per schema-bearing node), and `DispatchBudget`'s own doc stating it
already counts "every dispatch on every path (single, fan-out, fleet, pipeline
item, schema retry, loop iteration)" — which is precedent in this codebase for
one budget owner spanning two loops, exactly the construction F22C requires.

## What was NOT done

- **Task 2** (cross-audited decision on how much observable behavior change is
  authorized to subordinate five engines): not run. No panel was convened.
- **Task 3** (`crates/wcore-agent/src/goal/strategy.rs` — the adapter surface
  that makes terminating outside the canonical transition *structurally*
  impossible, the explicit loop-owner claim, the nesting refusal): **not built.**
  This is the substance of Success Criterion 3 and it does not exist.
- **Task 4** (drive all five engines through the real shipped binary and prove
  one canonical terminal transition each): not run.
- `crates/wcore-agent/tests/goal_strategy_test.rs` and
  `goal_strategy_live_test.rs`: do not exist.

## Requirements

**F22-04 — NOT complete.** The census is the input to the requirement, not the
requirement.

## Honest read

The census is genuinely valuable and it corrected the plan's own central
assumption before any code was written, which is what a census is for. But
Success Criterion 3 asks that five engines *terminate through* one transition,
and after this plan they still terminate through five. A taxonomy that everything
*could* map onto is not the same as a construction where nothing can terminate
any other way, and the plan was explicit that a convention is not the property.
