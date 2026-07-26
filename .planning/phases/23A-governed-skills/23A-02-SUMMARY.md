---
phase: 23A-governed-skills
plan: "02"
subsystem: governed-skills
tags: [not-executed, blocked]
requires: [23A-01]
provides: []
key-files:
  created: []
  modified: []
metrics:
  completed: 2026-07-26
status: not_started
---

# Phase 23A Plan 02: Governed Promotion Transaction — NOT EXECUTED

**Disposition: NOT STARTED. No code was written, no partial state exists on disk.**

This is an explicit incomplete disposition, not a silent omission.

## Why

23A-01 did not reach a green live proof. Its terminal defect, **F23A-01-H2**, is that
*any* errored tool call kills the session — reproduced on Linux with three independent
triggers, including a `Read` of a nonexistent path, so it is not a skills defect at all.

23A-02's own objective is to replace the currently-suspended `run_skills_promote` with a
governed transaction and then **prove the before-and-after through the shipped binary on
Linux and Windows** (its Task 3). That proof runs through the same live-drive path that
is currently red. Building a promotion transaction on top of a session that cannot
survive a refused tool call would produce a promotion path whose "before" state cannot be
demonstrated — the exact false-green shape this phase exists to prevent.

The plan's own premise says it: *"that transaction is only meaningful if the pre-promotion
state is genuinely inert."* The pre-promotion state **is** inert — 23A-01 established that
across sixteen routes. What is not established is that the product can be *driven* to
demonstrate it end to end.

## What is known that a later executor should not have to rediscover

- `run_skills_promote` is at `crates/wcore-cli/src/main.rs:2408` and currently fails
  closed: `"skill promotion is temporarily unavailable while governed promotion is being
  implemented"`. It rejects before UUID parsing, DB access or filesystem mutation.
- The sibling `run_skills_archive` (`main.rs:2417`) and the shared
  `transition_procedure` backend (`main.rs:2429`) show the exact open-memory → lookup →
  `can_transition_to` → upsert sequence a governed promote should mirror.
- **`crates/wcore-cli/src/main.rs` is a FENCED file in this wave.** The governance ledger
  and the transaction itself belong in `wcore-skills` per AGENTS.md ("lowest crate where
  it semantically belongs"); only the CLI dispatch wiring needs a `main.rs` edit, and that
  edit must be filed as a seam request, not made directly.
- The live harness to use is `run_with_binary_in_paths` + `OpenAiFixtureScript`, already
  proven in `packaged_driver_gate.rs` and now also in
  `crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs`. Do not build a bash driver:
  a `.sh` cannot stand up an OpenAI-compatible fixture or provoke `PatternDetector`.

## Requirement status

**F23-01 is NOT marked complete.** Promotion does not exist; the requirement's
`promote, observe, revoke, rollback` clauses are entirely unmet.
