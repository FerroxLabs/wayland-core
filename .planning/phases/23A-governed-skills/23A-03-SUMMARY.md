---
phase: 23A-governed-skills
plan: "03"
subsystem: governed-skills
tags: [not-executed, blocked]
requires: [23A-02]
provides: []
key-files:
  created: []
  modified: []
metrics:
  completed: 2026-07-26
status: not_started
---

# Phase 23A Plan 03: Observe, Revoke, Rollback — NOT EXECUTED

**Disposition: NOT STARTED. No code was written, no partial state exists on disk.**

## Why

23A-03 revokes and rolls back a **promoted** skill. Promotion does not exist: 23A-02 was
not executed and `run_skills_promote` still fails closed. There is nothing to revoke and
nothing to roll back to. Executing this plan would have meant inventing the promoted state
it operates on, which is the hand-seeding this phase's plans forbid by name.

Its Task 3 additionally re-runs 23A-01's route-refusal assertion set against a *revoked*
artifact through the shipped binary — and that assertion set is currently red on
**F23A-01-H2** (any errored tool call kills the session), so the re-run could not have
produced a meaningful result either.

## What 23A-03 does NOT block, and what it does

The **observe** half is partially satisfied already and that is worth recording, because
it is the part of Success Criterion 1 that the product genuinely does today:

- `/skill list` shows a quarantined generated draft, tagged `(hidden)`, with a
  visible/hidden summary (`crates/wcore-agent/src/slash/skill.rs:145-168`).
- `/skill show <name>` reports `visibility: hidden from model` along with source,
  `loaded_from` and file path, and **does not render the body**
  (`crates/wcore-agent/src/slash/skill.rs:171-208`).

Both were observed at the product surface via the repository's own packaged lifecycle
matrix. So an operator *can* see what is quarantined. What does not exist is governance
provenance, append-only history, revoke, or rollback — the rest of the criterion.

## Requirement status

**F23-01 is NOT marked complete.** The `observe` clause is partially met (visibility only,
no provenance or history); `revoke` and `rollback` are entirely unmet.
