# Phase 23B — Continuous Agency: phase disposition

**Status: PARTIAL. One plan of four executed. The phase goal is NOT achieved.**

This document grades Phase 23B honestly against `.planning/ROADMAP.md`. Plans 23B-02,
23B-03 and 23B-04 were **not executed at all**. No SUMMARY was written for them,
deliberately: a SUMMARY marks a plan done, and writing one for work that did not happen
is the failure mode this program has spent two phases learning to avoid.

---

## Plan status

| Plan | Wave | Status |
|---|---|---|
| 23B-01 — session operator lifecycle (F23-02) | 1 | **EXECUTED.** Complete with named open verbs. See `23B-01-SUMMARY.md`. |
| 23B-02 — memory provenance, control, cache/compaction/cost truth (F23-03, F23-04) | 2 | **NOT EXECUTED.** No code, no tests, no evidence. |
| 23B-03 — persistent hybrid repository index (F23-06) | 3 | **NOT EXECUTED.** No code, no tests, no evidence. |
| 23B-04 — multi-day journey, clock policy, aggregate proof (F23-05) | 4 | **NOT EXECUTED.** No decision run, no journey, no aggregate. |

The plans are strictly wave-ordered by real content dependencies (23B-02 and 23B-03
both edit `crates/wcore-cli/src/tui/commands/mod.rs`; 23B-04 consumes all three
SUMMARYs). Executing them out of order was not an option, and there was not time to
execute them in order.

---

## Success Criteria — verbatim from `.planning/ROADMAP.md` Phase 23, graded

> **1. Generated skills cannot execute before governed promotion and can be observed,
> revoked, and rolled back.**

**NOT 23B's.** Owned by Phase 23A and an admitted input here. Not graded.

> **2. Users can search, inspect, checkpoint, retry, fork, rewind, export, retain, and
> reconcile session effects.**

**PARTIAL — the strongest result of this phase.**

Every listed verb, plus `cancel`, is reachable on the shipped `wayland-core` binary and
was driven from a real command line with a captured observable outcome **on Linux**.
Export provably omits a run-time-generated nonce. Rewind restores byte-identical
content and removes a file created after the checkpoint. Fork leaves the parent
byte-identical. Checkpoint restore refuses a destination outside the workspace root and
writes nothing.

Critically, **the criterion's `reconcile` clause — and the `cancel` verb the engine's
own error message names — did not exist in the product before this plan.** That is
live Windows UAT defect D2, and it is now closed end to end: a genuinely
crash-interrupted session refuses `--resume`, `session reconcile` names the blocking
item, the operator resolves it, `session cancel` succeeds, the disposition survives a
restart, and `--resume` stops refusing.

Not met: macOS and Windows were not driven, the TUI verbs were not added or driven, and
`retry` is live-proved only on its refusal path.

> **3. Users can see and control memory/user-model activation, provenance, correction,
> forgetting, privacy, retention, and nudges.**

**NOT MET.** 23B-02 was not executed. Nothing was built. `/memory` still offers only
show and clear; recall provenance does not exist; there is no correction, forgetting,
privacy, retention or nudge control. F23-03 is untouched.

> **4. Cache and compaction behavior expose quality, invalidation, token-pressure, and
> cost truth.**

**NOT MET.** 23B-02 was not executed. `cache_diagnostics.rs` still emits telemetry
only; there is no operator-facing invalidation cause, token-pressure report, compaction
quality verdict or cost reconciliation. F23-04 is untouched.

> **5. A multi-day wait/resume/complete journey preserves cumulative authority,
> resource, evidence, memory, and delivery state.**

**NOT MET.** 23B-04 was not executed. No clock-policy decision was cross-audited, no
journey was run. F23-05 is untouched.

Worth recording: finding 23B-H1 below is directly adversarial to this criterion. A
multi-day journey depends on resuming a session across days, and the product currently
fails to resume a large fraction of sessions it wrote minutes earlier.

> **6. A persistent incremental hybrid repository index provides bounded
> lexical/symbol/optional-semantic retrieval with provenance, staleness, privacy and
> performance truth.**

**NOT MET.** 23B-03 was not executed. `wcore-repomap` remains the ~1,300-line in-memory
crate it was: no persistence, no incremental update, no content hashing, no full-text
search, no ranking, no staleness, no provenance, no perf gate. F23-06 is untouched.

---

## Requirements

| Requirement | Disposition |
|---|---|
| F23-02 | **INCOMPLETE.** Substantial delivery on Linux; unmet clauses named in `23B-01-LIVE-EVIDENCE.md` §5. |
| F23-03 | **INCOMPLETE — not started.** |
| F23-04 | **INCOMPLETE — not started.** |
| F23-05 | **INCOMPLETE — not started.** |
| F23-06 | **INCOMPLETE — not started.** |

None is marked complete.

---

## The finding that matters most

**23B-H1 (HIGH, pre-existing):** a cleanly-exited `wayland-core` run can write a session
journal the product cannot read back. `--resume` fails with
`journal checksum mismatch at sequence 16` on a session `--list-sessions` still shows,
and every operator verb that reads the journal fails identically, so there is no repair
path. Measured 8/8 and 9/10 in two bursts under concurrent compile load on
`hetzner-dsm`, 0/3 when the host was quiet.

This is strictly worse than the defect Phase 23B was chartered to close. D2 leaves a
session blocked but repairable — which 23B-01 now repairs. 23B-H1 leaves it
permanently unreadable, and it undercuts Criterion 5 before that criterion is even
attempted.

**Confirmed pre-existing.** The build host was reverted to untouched base sources and
`wayland-core` rebuilt; the binary was verified pristine by `session --help` exiting
non-zero (this phase's subcommand absent from it). Driving only the engine's own
`--resume` path, the defect reproduced **9/10**. It is not a Phase 23B regression.

Full evidence: `23B-01-LIVE-EVIDENCE.md` §3. Filed for BACKLOG as SR-23B-05.

---

## Escalations to Sean

1. **Instruction conflict on the macOS leg.** All four 23B plans decide, with recorded
   measurements, that the macOS leg builds its own binary on this Mac
   (`scripts/f23-macos-binary.sh`), citing HANDOFF §3 item 7. The controlling execution
   instruction for this phase says never to run Cargo on the Mac, `cargo fmt` excepted.
   These cannot both hold. I honoured the controlling instruction, so every macOS row
   in this phase is OPEN and `scripts/f23-macos-binary.sh` does not exist — which also
   blocks 23B-02, 23B-03 and 23B-04, all of which consume that script unchanged.
   One of the two must be relaxed before any macOS row in Phase 23B can close.

2. **23B-H1 is confirmed pre-existing (9/10 on a pristine baseline binary) and needs a
   tracked issue**, not a backlog row. Opening the issue is reserved to Sean.

3. **Build-host disk exhaustion.** `hetzner-dsm` reached 93% with six phases building
   concurrently; the full `-p wcore-agent -p wcore-cli` aggregate failed with
   `No space left on device` rather than any code result. Targeted suites and workspace
   clippy completed before that point.
