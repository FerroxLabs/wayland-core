# Phase 23B — Continuous Agency: phase disposition, revision 2

> ## SUPERSEDED on the plan-count claim — 2026-07-28
>
> This revision's statement that "23B-03 and 23B-04 were not executed at all, and no
> SUMMARY was written for them" was true when written and is **now false**. Both have
> since executed and both have summaries:
>
> - **23B-03** — complete. F23-06's persistent incremental index, live-proven on all three
>   platforms; the OPTIONAL semantic/RRF layer is deferred and reports its own
>   unavailability rather than being half-wired. See `23B-03-SUMMARY.md`.
> - **23B-04** — Task 1 complete, Task 2 started, **Task 3 deliberately unstarted**. Day one
>   of the multi-day journey is recorded on Linux and Windows; macOS is NOT ACHIEVED. The
>   journey **cannot close before 2026-07-30T23:54:26Z** — a real elapsed-time floor, not a
>   scheduling estimate. See `23B-04-SUMMARY.md` and `23B-04-JOURNEY-HANDOFF.md`.
>
> The phase goal remains **NOT achieved**, and the reasoning below still holds on every
> other point. Only the plan-count sentence is superseded.

**Status: PARTIAL. Two plans of four executed. The phase goal is NOT achieved.**

Revision 1 (`23B-PHASE-DISPOSITION.md`) graded the phase after 23B-01. This revision adds
the 23B-02 lane and the disposition of finding 23B-H1. **23B-03 and 23B-04 were not
executed at all, and no SUMMARY was written for them** — a SUMMARY marks a plan done, and
writing one for work that did not happen is the failure mode this program has spent three
phases learning to avoid.

---

## Plan status

| Plan | Wave | Status |
|---|---|---|
| 23B-01 — session operator lifecycle (F23-02) | 1 | **EXECUTED.** Complete with named open verbs. Re-proved live on the current lane HEAD (see below). |
| 23B-02 — memory provenance and control; cache/compaction/cost truth (F23-03, F23-04) | 2 | **PARTIALLY EXECUTED.** Task 1 delivered and surfaced; Task 2 not started; Task 3 not written. See `23B-02-SUMMARY.md`. |
| 23B-03 — persistent hybrid repository index (F23-06) | 3 | **NOT EXECUTED.** No code, no tests, no evidence. |
| 23B-04 — multi-day journey, clock policy, aggregate proof (F23-05) | 4 | **NOT EXECUTED.** No decision run, no journey, no aggregate. |

---

## Success Criteria — graded

> **1. Generated skills cannot execute before governed promotion…**

**NOT 23B's.** Phase 23A owns it. Not graded here.

> **2. Users can search, inspect, checkpoint, retry, fork, rewind, export, retain, and
> reconcile session effects.**

**PARTIAL — and now re-proved on current code.** 23B-01's fifteen verb rows were driven
again against a binary built at the exact lane HEAD (`--build-info` reports
`source cd021a01…`, matching the commit under test), with a caller-generated run nonce:

```
F23_01_DRIVE=PASS platform=linux nonce=c3ebab28a4160e31      (driver exit 0)
```

**Live Windows UAT defect D2 is closed end to end, again, on the fixed code.** A session
was genuinely crash-interrupted — a turn driven against a socket that accepts and never
answers, then `kill -9` mid-dispatch — and then:

| Step | Marker |
|---|---|
| the crash produced a real interrupted turn | `F23_01_D2_FIXTURE_INTERRUPTED=true` |
| `--resume` refuses, naming the interrupted turn | `F23_01_D2_REFUSAL_OBSERVED=true` |
| `session reconcile` names the blocking item | `F23_01_D2_RECONCILE_ITEMS_REPORTED=1` |
| the operator resolves it | `F23_01_D2_RECONCILE_RESOLVED=1` |
| `session cancel` succeeds | verb `cancel` PASS, exit 0 |
| the disposition survives a fresh process | `F23_01_D2_RESOLVED_PERSISTS_ACROSS_RESTART=true` |
| `--resume` stops refusing | `F23_01_D2_CONTINUE_UNBLOCKED=true` |

That is the criterion's `reconcile` clause and the `cancel` verb the engine's own error
message named, driven on the real binary. **Still not met:** macOS and Windows were not
driven, and the TUI verbs were not added.

> **3. Users can see and control memory/user-model activation, provenance, correction,
> forgetting, privacy, retention, and nudges.**

**PARTIAL, and NOT met.** Recall provenance now exists and is emitted by the fusion that
produced the ranking. Correction, forgetting, privacy scoping and retention bounding run
through the unmodified access gate, are audited, and are reachable from `/memory` on the
shipped surface. A forget reaches the CDC changelog. Exclusions are reported rather than
silent.

It is not met, for reasons stated rather than glossed:

- **The plan's own acceptance mechanism was not used.** F23-03's demand is that forgetting
  be proved by absence from the ACTUAL OUTBOUND PROVIDER REQUEST BODY. What exists is a
  proof that the row is deleted and gone from retrieval. The plan explicitly names
  "asserting a deleted row" as the engineered green to avoid. On this evidence the
  criterion cannot be called met.
- Nothing was driven live. No TUI leg on any platform.
- User-model correction precedence was not implemented; the nudge bound exists but no
  command exposes it.

> **4. Cache and compaction behavior expose quality, invalidation, token-pressure, and
> cost truth.**

**NOT MET.** 23B-02 Task 2 was not started. `cache_diagnostics.rs` still emits telemetry
only. F23-04 is untouched.

> **5. A multi-day wait/resume/complete journey preserves cumulative authority, resource,
> evidence, memory, and delivery state.**

**NOT MET.** 23B-04 was not executed. No clock-policy decision, no journey, no aggregate.

Revision 1 recorded that finding 23B-H1 was directly adversarial to this criterion. That
obstacle is now removed on the write path — see below — but the criterion itself is
untouched.

> **6. A persistent incremental hybrid repository index…**

**NOT MET.** 23B-03 was not executed. `wcore-repomap` is unchanged.

---

## Requirements

| Requirement | Disposition |
|---|---|
| F23-02 | **INCOMPLETE.** Substantial delivery on Linux, re-proved on current code. macOS, Windows and TUI legs open. |
| F23-03 | **INCOMPLETE.** Substrate and surface delivered; not proved against an outbound prompt; not driven live. |
| F23-04 | **INCOMPLETE — not started.** |
| F23-05 | **INCOMPLETE — not started.** |
| F23-06 | **INCOMPLETE — not started.** |

None is marked complete.

---

## 23B-H1 — the phase's open HIGH is closed on the write path

Full record: `23B-H1-DISPOSITION.md`.

The defect is a **serialization round-trip instability**, not a torn write. Both
`effect_receipt` fields are `Option<serde_json::Value>` with
`skip_serializing_if = "Option::is_none"`, so `Some(Value::Null)` writes an explicit
`"effect_receipt":null`, decodes back to `None`, and re-serializes to nothing. The
recomputed checksum covers different bytes and the reader rejects a journal the writer
wrote correctly — permanently, since every operator verb reads the journal.

Reproduced deterministically and fixed. Red before green, by reverting only the predicate
and keeping the tests: **2 failed / 3 passed** before, including
`ChecksumMismatch { seq: 1 }` end to end through the real writer and a fresh reader;
**5 passed / 0 failed** after. No regression across
`session_journal` (66), `session_journal_test` (48), `session_journal_crash_matrix_test` (4).

**Two things this does not claim.**

1. **The residual.** The fix closes the write path. A journal already on disk carrying an
   explicit null still fails its checksum, and that content is lost. Teaching the
   integrity check to accept two encodings is a worse trade and was not done.
2. **I could not reproduce 23B-H1's original symptom live: 34 runs, 0 reproductions**,
   including 12 against the pristine `15971d1b` binary at a one-minute load average of
   **79 → 130**, well above the 28 that 23B-01 recorded. This does **not** disprove 23B-01's
   measurement. It narrows the trigger: my harness drives a turn against a closed port, so
   dispatch fails before any tool runs and the reconciler-bearing tool intent that carries
   the defective shape is never written. Load alone is ruled out.

---

## Escalations to Sean

1. **Instruction conflict on the macOS leg — UNCHANGED and still blocking.** All four 23B
   plans decide that the macOS leg builds its own binary on this Mac via
   `scripts/f23-macos-binary.sh`. The controlling execution instruction for this phase
   forbids running Cargo on the Mac, `cargo fmt` excepted. I honoured the controlling
   instruction, as 23B-01 did. Every macOS row in this phase remains OPEN and that script
   still does not exist.
2. **23B-H1 needs a tracked issue**, both for the fix and for the residual: any session
   whose journal already carries an explicit null receipt is unreadable and unrecoverable.
   Opening the issue is reserved to Sean.
3. **23B-03 and 23B-04 remain entirely unexecuted.** Criteria 4, 5 and 6 have had no work
   at all across two lanes.
