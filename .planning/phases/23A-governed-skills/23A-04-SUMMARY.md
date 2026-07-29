---
phase: 23A-governed-skills
plan: "04"
subsystem: governed-skills
tags: [not-executed, blocked, phase-disposition]
requires: [23A-01, 23A-02, 23A-03]
provides:
  - phase disposition for 23A
key-files:
  created: []
  modified: []
metrics:
  completed: 2026-07-26
status: not_started
---

# Phase 23A Plan 04: Journey, macOS Decision, Phase Disposition — NOT EXECUTED

**Disposition: NOT STARTED as written.** Its Task 1 (the one-run governed-skill journey)
and Task 2 (the macOS coverage decision) were not performed. Its Task 3 obligation — to
state the phase disposition honestly — is discharged here, because that obligation does
not depend on the other two.

## Why Tasks 1 and 2 were not executed

**Task 1** drives one continuous journey: detect → draft → quarantine → evaluate →
review → promote → observe → revoke → rollback. Six of those nine stages do not exist,
because 23A-02 and 23A-03 were not executed. A "journey" through three implemented stages
and six absent ones is not a journey; it is 23A-01's route drive relabelled.

**Task 2** is a cross-audited decision about what macOS evidence is obtainable without a
Sean-gated ephemeral runner dispatch. Running that panel would have been cheap, and I did
not run it — but its output authorises platform coverage for a proof that does not yet
pass on Linux. Deciding which platforms to certify a red proof on is not a decision worth
four models' time. **Recorded as not done, with the reason, rather than performed
ceremonially to produce an artifact.**

Note for whoever picks this up: the premise that no macOS binary is obtainable is **false
and must not be reused** — CI has uploaded `wayland-core-aarch64-apple-darwin` per-target
binaries since `d9c7683b`. "Unobtainable" is not a valid basis for closing a leg on Linux
alone.

---

## Phase 23A disposition — Success Criterion 1, graded verbatim

> **"Generated skills cannot execute before governed promotion and can be observed,
> revoked, and rolled back."**

The criterion is a conjunction of four clauses. Graded one at a time.

### Clause 1 — "cannot execute before governed promotion": **MET on the code paths that exist, with a caveat**

Sixteen routes enumerated with citations, all sixteen gated
(`23A-01-SURFACE-CENSUS.md`). Observed at the product surface: a `Skill` tool call naming
a generated draft returns `is_error` with `not found` and does **not** disclose the draft
body.

Caveat that keeps this from being a clean pass: the phrase *"before governed promotion"*
presupposes governed promotion, and there is none. The pre-promotion state is inert
because it is **permanently** inert — `run_skills_promote` fails closed
(`crates/wcore-cli/src/main.rs:2408`). "Cannot execute before promotion" is currently
satisfied by the absence of a promotion path, not by a governance boundary. That is the
same vacuous-truth shape as the F21-02 finding and it is stated rather than counted as a
pass.

### Clause 2 — "can be observed": **PARTIALLY MET**

`/skill list` tags the draft `(hidden)`; `/skill show` reports
`visibility: hidden from model` and withholds the body. Both observed live. What does not
exist: governance provenance and append-only history.

**And an unqualified caveat:** observation is degraded in practice by **F23A-01-H2** — a
model that tries a quarantined skill kills the session. The operator can inspect the
quarantine, but the refusal that enforces it is not survivable.

### Clause 3 — "can be revoked": **NOT MET.** Nothing implements revocation.

### Clause 4 — "can be rolled back": **NOT MET.** Nothing implements rollback.

### Overall: **SUCCESS CRITERION 1 IS NOT MET.**

Two of four clauses are unmet outright, one is partial, and the one that is met is met in
part by the absence of the feature it is defined relative to.

---

## What the phase produced that is worth more than the criterion it missed

Two HIGH defects, neither of which any amount of code reading would have produced, both
found by driving the shipped binary:

1. **F23A-01-H1** — a documented authority boundary (`skills_lifecycle = false`) failing
   OPEN in the default configuration of every freshly cloned project, leaking
   project-derived skill drafts into the global skills directory. **Fixed**, with a 9-cell
   live control matrix and a red/green unit regression. The pre-existing unit suite never
   caught it because every test covered only the trusted path.

2. **F23A-01-H2** — **any** errored tool call kills the session. Reproduced in 13 seconds
   with three independent triggers, one of which (`Read` on a missing path) proves it is
   not a skills defect at all. Breaks the repository's own packaged product gate.
   **Left open and committed red**, with the ownership question settled by measurement.

The second one is the more important output of this phase, and it was only reachable
because the first was fixed — H1 was masking it.

---

## Requirement status

**F23-01 is NOT marked complete**, and no other F23 requirement is touched. The unmet
clauses are: `evaluate`, `review/policy`, `promote`, `revoke`, `rollback`, and the
provenance/history half of `observe`.

---

> **STATUS CORRECTION (2026-07-29, lane/record-truth).** This document records
> `F23A-01-H2` as open. **It was fixed at `32a5fc90` on 2026-07-27**, with five
> wired regression tests in
> `crates/wcore-agent/src/orchestration/d1_refusal_terminal_tests.rs`. The body
> above is left as written on purpose. See `23A-STATUS-CORRECTION.md` in this
> directory for the evidence — and for the gap underneath it: the 16-route
> quarantine census is **still unmeasured at HEAD**, and `WAYLAND_F23A_SELFTEST`
> was never shown to fire. H2 being fixed does not close the census.
