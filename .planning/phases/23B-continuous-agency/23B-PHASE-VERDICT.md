# Phase 23 (23A + 23B) — PHASE VERDICT

**Graded by:** lane `grade-23b`, 2026-07-29T10:03Z
**Base:** `861d1b1a`  ·  **Branch:** `lane/grade-23b`
**Scope:** all six Phase 23 Success Criteria in `.planning/ROADMAP.md:101-107`.
Criterion 1 belongs to Phase 23A (ROADMAP:109) and is graded here from in-tree evidence only,
with authority deferred to the concurrent 23A lane.

**Working notes with every raw measurement:** `23B-GRADE-NOTES.md` (same directory).

---

## VERDICT: **NOT ACHIEVED**

| # | Criterion | Grade |
|---|---|---|
| 1 | Generated skills cannot execute before governed promotion; observable, revocable, rollback-able | **PARTIAL** *(23A's — deferred)* |
| 2 | Search, inspect, checkpoint, retry, fork, rewind, export, retain, reconcile session effects | **PARTIAL** |
| 3 | See and control memory/user-model activation, provenance, correction, forgetting, privacy, retention, nudges | **NOT MET** |
| 4 | Cache and compaction expose quality, invalidation, token-pressure, cost truth | **NOT MET** |
| 5 | Multi-day wait/resume/complete journey preserves cumulative state | **NOT MET** |
| 6 | Persistent incremental hybrid repository index | **MET WITH STATED EXCEPTIONS** |

**1 met, 2 partial, 3 not met.** The phase goal — "the agent can pursue verified outcomes over
time, learn safely, and let users inspect, correct, recover, and control that state" — is not
achieved and should not be recorded as achieved.

**Two corrections to the inherited picture, both from live measurement, and they run in opposite
directions.** Neither is in any artifact in this tree:

1. **The Linux multi-day journey leg is DEAD, not running.** It failed on 2026-07-28 and has
   recorded one day of three. It will fail again tomorrow. See C5 and Gap G5a — this is the most
   time-critical item in the phase.
2. **Windows reached day 2 successfully** on 2026-07-28T23:58:17Z. The brief's "day one only" is
   now true of Linux and macOS, not Windows.

---

## Criterion 1 — governed skills — **PARTIAL** (23A's; deferred)

**Evidence:** `.planning/phases/23A-governed-skills/23A-C1-SUMMARY.md:4, :20-23`.

23A's own C1 lane grades it: `observed` MET, **`revoked` MET**, **`rolled back` MET**,
`cannot execute before promotion` met *and explicitly not vacuously* — and **`promoted` STILL NOT
MET** (`bail!`, untouched).

**Missing:** governed promotion itself. **Cost:** the criterion's central verb is absent, so a
generated skill can be observed and destroyed but never legitimately admitted — the capability the
phase exists to deliver.

`.planning/REQUIREMENTS.md:383` is **stale** on this criterion: it still records "`revoke` and
`rollback` unimplemented and clause 1 satisfied only vacuously", which `23A-C1-SUMMARY.md`
supersedes. No `*-VERDICT.md` or `*-DISPOSITION.md` exists in `23A-governed-skills/`.
**Authority for this row belongs to the 23A lane.**

---

## Criterion 2 — session operator lifecycle — **PARTIAL**

**Met, and live-proven.** All fifteen operator verbs — list, search (hit), search (miss), show,
checkpoint, rewind, fork, show-lineage, retry, export, retain, retain-expired, reconcile,
reconcile-resolve, cancel — pass against the shipped binary on Linux:

```
evidence/23B-02-linux-operator-drive.log
F23_01_PROVENANCE=ok platform=linux sha=cd021a011ddfc0387d840530d368ae6f3a916b9e
F23_01_DRIVE=PASS platform=linux nonce=c3ebab28a4160e31
```

plus the live Windows-UAT D2 chain end to end (`D2_FIXTURE_INTERRUPTED`, `D2_REFUSAL_OBSERVED`,
`RECONCILE_ITEMS_REPORTED=1`, `RECONCILE_RESOLVED=1`, `cancel` exit 0,
`D2_RESOLVED_PERSISTS_ACROSS_RESTART=true`, `D2_CONTINUE_UNBLOCKED=true`).

Artifacts exist and are substantial: `session_lifecycle.rs` (1297 lines),
`session_cmd.rs` (499), `session_operator_lifecycle.rs` (534).

**Provenance correction — re-derived, not inherited.** `23B-01-LIVE-EVIDENCE.md:18-22` publishes
its fifteen PASS rows against run nonce `446156892e72cf2a` and points at
`evidence/23B-01-linux-drive.log`. **That log is a different run and it FAILED**
(`F23_01_DRIVE=FAIL … failures=4`, nonce `f12f77104c3ca039`, show/retry/export/reconcile all dead
on `journal checksum mismatch at sequence 16`). None of the three nonces 23B-01 cites appears
anywhere in `evidence/` — 0 hits each, against known-positive controls returning 1 hit each in the
same invocation. **23B-01's own acceptance log is not in the tree.** The criterion survives on the
23B-02 re-drive above, whose binary provenance is strictly better: `git ls-tree cd021a01`
contains all three phase files, while `git ls-tree 15971d1b` (23B-01's binary) contains **none**,
confirming 23B-01's own stated rsync caveat.

**Missing, and what it costs:**

- **No macOS leg and no Windows leg — and the Windows one is unbuilt, not merely unrun.**
  `scripts/f23-session-operator-drive.ps1` does not exist (`ls scripts/f23*` returns 15 scripts,
  none of them a session-operator PowerShell port). Cost: the operator surface is proven on one
  of three supported platforms.
- **No TUI leg on any platform.** `/checkpoint`, `/fork`, `/export` were never added and no PTY
  run occurred. Cost: an operator on the product's primary interactive surface cannot reach these
  verbs at all.
- **The 23B-H1 residual is live and I watched it bite.** `BACKLOG.md` records that an unreadable
  journal has no repair path and that all twelve `session` verbs read the journal. The failing
  `23B-01-linux-drive.log` is that finding in action: **one checksum mismatch took out 4 of 15
  verbs in a single run.** Cost: the recovery surface's own failure mode is unrecoverable.

**Confidence:** high on Linux (nonce-bound, provenance-asserted, driven against the shipped
binary); the criterion is nonetheless PARTIAL on platform and surface coverage.

---

## Criterion 3 — memory and user-model control — **NOT MET**

**What exists.** `provenance.rs` (989 lines) and schema `v6_recall_control.sql`.
`/memory why|correct|forget|privacy|retention` are genuinely wired
(`slash/memory.rs:54-58` dispatch; `:97,160,188,204,253` implement). Controls run through the
unmodified `MemoryAccessGate`, are audited, and a forget reaches the CDC changelog. Exclusions are
reported rather than silent.

**Why it is NOT MET — measured independently, and matching 23B-02's own honest self-grade:**

- **The plan's central acceptance mechanism was never used.** F23-03 (`REQUIREMENTS.md:120-121`)
  turns on forgetting being proved by absence from the **actual outbound provider request body**.
  `received_requests` in `crates/wcore-memory` + `crates/wcore-agent/src/slash` → **0 files**;
  known-positive, same query in `crates/wcore-providers` → **7 files**. The instrument is alive and
  the proof is genuinely absent. What exists proves a deleted row — which 23B-02-PLAN names by hand
  as the engineered green to avoid. **Cost: "forgotten" is unproven where it matters — in the
  prompt.**
- **Nudges are not reachable.** `NudgeBudget` occurs only in `wcore-memory/src/lib.rs` and
  `provenance.rs`; no CLI, slash or TUI surface. The criterion names nudges explicitly.
- **Nothing was driven live on any platform**, and `crates/wcore-cli/tests/memory_control_lifecycle.rs`
  does not exist. Against this program's standing rule that live testing ranks at least as high as
  green code, tests alone cannot carry this row.
- **User-model correction precedence was not implemented.** The criterion says "memory/user-model".

---

## Criterion 4 — cache and compaction truth — **NOT MET**

**Phase 23 contributed nothing to this criterion.** 23B-02 Task 2 was never started
(`23B-02-SUMMARY.md`, `requirements_disposition: F23-04: not-started`), and I confirmed that
independently rather than inheriting it:

- `grep -rln "F23_04\|F23-04" crates/ scripts/` → 5 files, **all five belonging to plan 23B-04
  (the multi-day journey)**. Note the numbering collision: plan `23B-04` implements requirement
  **F23-05**, not F23-04. There is no F23-04 artifact.
- Concept search (quoted globs) for `invalidation_cause|InvalidationCause|token_pressure|
  TokenPressure|cost_reconcil|CostReconcil|compaction_quality|CompactionQuality` → 20 hits, **all
  in `crates/wcore-providers/src/cache_observation.rs` (19) and `lib.rs` (1)**, a pre-existing file
  last touched by PR #186 ("cache-health **telemetry**"). `cache_diagnostics.rs` last touched
  `58e64fc6` (2026-07-15, pre-23B); `compact/state.rs` last touched PR #65.
  *(My first attempt returned 0 because zsh ate the unquoted `--include=*.rs`. Discarded, not
  reported — the exact self-passing-negative trap the lane brief names.)*

**Against the four clauses the criterion demands:**

| Clause | User-visible? | Evidence |
|---|---|---|
| cost truth | **partly, and pre-existing** | `/cost` TUI surface, session spend + per-turn breakdown (`tui/surfaces/diagnostics.rs:789,1890-1903`) — predates Phase 23 entirely |
| cache quality / invalidation | **no** | `CacheHealthWarn` at `engine.rs:11081` is, in the code's own words, *"Warning-only structured telemetry: greppable in the engine log, never alters the request"* |
| token-pressure | **no** | `TokenPressure` has **zero** references in `wcore-agent` or `wcore-cli` |
| cost-regression thresholds | **no** | none exist |

Graded NOT MET rather than PARTIAL deliberately: one of four clauses has a partial surface, that
surface predates this phase, and crediting it would attribute to Phase 23 work Phase 23 did not do.

---

## Criterion 5 — the multi-day journey — **NOT MET**

**A journey that has not elapsed cannot be graded complete, and this one has not elapsed.**
Beyond that, the leg the tree believes is running is not running.

**Live state, measured on the hosts at 2026-07-29T10:03Z:**

| Platform | Days recorded | State |
|---|---|---|
| Linux (`hetzner-dsm`) | **1 of 3** | **BROKEN.** Day 2 failed 2026-07-28T14:25:00Z |
| Windows (`SEANDESKTOP`) | **2 of 3** | On track; day 3 scheduled 2026-07-31 07:05 local |
| macOS | **0 of 3** | Never started; blocked on the compiled test harness |

```
/root/.f23-journey-linux/scheduled.log
  scheduled day 1 exited 0 at 2026-07-27T14:40:03Z
  scheduled day 2 exited 1 at 2026-07-28T14:25:00Z      <-- FAILED
grep -c "F23_04_DAY=" /root/.f23-journey-linux/runlog.txt  ->  1
/root/.f23-journey-linux/scheduled-day2.log
  scripts/f23-multi-day-journey.sh: line 67: HOME: unbound variable

C:\Users\seand\.f23-journey-windows\  ->  2 day rows
  F23_04_DAY=1 … ts=2026-07-27T23:54:26Z host=SEANDESKTOP pid=39008
  F23_04_DAY=2 … ts=2026-07-28T23:58:17Z host=SEANDESKTOP pid=28584
```

**Root cause, proved rather than asserted.** `scripts/f23-multi-day-journey.sh:28` sets
`set -uo pipefail`; line 67 reads `[ -n "$ROOT" ] || ROOT="$HOME/.f23-journey-$PLATFORM"`. A
systemd transient service does not export `HOME`, so the script aborts before doing any work;
`/root/f23-journey-day.sh` passes no `--root`. Reproduced with three assertions in isolation
(known-negative fails rc=1 matching the observed `exited 1`; known-positive passes; the proposed
fix passes) **and on the actual host under the actual mechanism** — `systemd-run --wait --pipe`
running line 67 verbatim prints `HOME: unbound variable`, while the identical code in an ssh shell
prints `ROOT=/root/.f23-journey-linux`.

**This is the same defect class the lane guarded on Windows and not on Linux.** 23B-04's TRAP 4
documents that a SYSTEM scheduled task's `USERPROFILE` differs, and the Windows resume therefore
passes `-Root` explicitly. Linux got no equivalent guard.

**It recurs in ~28 hours.** `f23-journey-day3.timer` is still armed for
`Thu 2026-07-30 14:31:00 UTC` and calls the same unguarded script, so day 3 will fail identically
and the Linux leg will finish the window with one day of three.

**Nothing is lost.** The pinned binaries survive, the worktree is still detached at `0ed05322`,
and `/root/.f23-journey-linux/` retains `day-one.json` and `journey.journal`. The leg is
**recoverable**, and the repair is one line.

**Also open, and honestly recorded by 23B-04 itself:** finding 23B-04-M1 —
`BudgetWallClockAuthority::AbsoluteDeadline` has **zero production construction sites**, so the
absolute-deadline half of the criterion's own premise is presently unreachable by any user; and
23B-04-M2 — the journey's `memory-recall` invariant runs over the session journal, not
`wcore-memory`, so it does not evidence Criterion 3's subsystem.

**I did not repair the Linux leg.** This lane is fenced to grading, and injecting a day-2 row under
another lane's nonce would contaminate the evidence chain it belongs to. It is Gap G5a below and
it is the most time-critical item in the phase.

---

## Criterion 6 — persistent incremental repository index — **MET WITH STATED EXCEPTIONS**

This is the one criterion whose evidence survives adversarial reading intact, and the only place
in the phase where the instrument discipline was supplied unprompted.

**Proven on all three platforms through the shipped binary**, each run nonce-bound and each with
`F23_03_PROVENANCE=ok` asserted before any measurement:

| Platform | Nonce | Corpus path (platform identity) |
|---|---|---|
| Linux | `d3b14061fc7a3735` | `/root/wayland-23B-03` |
| macOS | `3a2127430e0437db` | `/private/var/folders/8h/…` (unforgeably Darwin) |
| Windows | `49a9ca44ae600fe8` | `//?/C:/Users/seand/…` (unforgeably Windows) |

All five content-hash mutations (add, edit, delete, rename, branch-switch) PASS on every platform
with `unchanged_reextracted=0`; `STALENESS_REPORTED=true`; `FALLBACK_REPORTED=true`;
`VERIFY exit=6` on drift. Artifacts are substantial: `store.rs` 1005, `search.rs` 543,
`scope.rs` 404, `index_cmd.rs` 201, plus 675+386 lines of tests. Suites: 58/58 Linux, 57/57
Windows, 1244/1244 with `wcore-tools` — all counts read back non-zero.

**The secret-isolation proof is a known-negative that carries its own known-positive in the same
run** — `STORE_NONCE_OCCURRENCES=0` alongside `STORE_CONTROL_OCCURRENCES=1` — which is exactly what
the brief's §3b-i demands and which nothing else in this phase supplies.

**Stated exceptions:**

1. **The OPTIONAL semantic / dense-vector layer was not built.** F23-06 marks it optional
   (`REQUIREMENTS.md:123`). The product reports its own unavailability via `semantic_status()` on
   every search and `index status`, and a test pins the string, so silent degradation to
   lexical-only would fail. This is a recorded non-claim, not a hidden gap.
2. **The legs ran at two SHAs.** Linux at `b33827d3`, macOS+Windows at `1eb2d7c2`. **Re-derived and
   benign:** `git diff --stat` between them over `crates/wcore-repomap`, `index_cmd.rs` and the
   drivers shows the **product code is identical**; only the driver scripts differ. The 11-line
   `.sh` delta is a field-extractor repair, so **the Linux leg ran the broken extractor** — visible
   in its log as `F23_03_VERIFY=agrees= exit=6`. I checked whether that vacated a Linux gate: it did
   not. `field verify agrees` is used at `f23-index-drive.sh:460` in an `echo` only; the gate at
   :461 reads `VERIFY_RC`, and every *asserted* extraction returned real non-empty values.
   **Cost: one informational field. Confidence downgrade: minor, not material.**
3. **Three open MEDIUMs**, all in `BACKLOG.md` and non-blocking under the phase's own severity
   rule: 23B-03-M1 (full-workspace `precision@1 = 0.8125`, recall@10 = 1.0000 — mis-ordered, not
   lost; the class the deferred semantic layer addresses), 23B-03-M2 (Windows `\\?\` fingerprint),
   23B-03-M3 (`instr()` fallback scan cost). 23B-03-L1: `cargo hakari verify` NOT RUN, tool absent.
4. **No perf pass-fail thresholds were invented.** Four figures recorded as a first baseline. The
   one absolute property gated — and chosen before measuring — is that a warm start opens **zero**
   files, and it holds on all three platforms.

---

## Instrument reliability — where confidence is downgraded, and where it need not be

**The two known-bad instruments named in my dispatch do NOT undermine this phase's evidence:**

- **`.config/nextest.toml`'s `no-tests = "fail"` being silently ignored:** not load-bearing here.
  **Every 23B plan gate passes `--no-tests=fail` on the command line**, not via the config
  (`23B-01-PLAN.md:158,193,194`; `23B-02-PLAN.md:166,200,201,243`; `23B-03-PLAN.md:170,171`), and
  23B-03 red-proved that the CLI flag works (exit **4** on a filter matching no test). Every
  reported suite also reads back a non-zero executed count (14/14, 19/19, 3418, 58/58, 57/57,
  1244/1244, 3 passed). **This phase is not exposed to the vacuous-suite class.**
- **`cargo nextest` "flakiness" being fd/inotify exhaustion:** the two red clusters in this phase
  were both correctly diagnosed as contention or pre-existing rather than laundered into green.
  23B-02's 14 raw-harness failures passed in isolation and `--test-threads=1` (2101 passed, 0
  failed); 23B-03's four `wcore-cli::child_authority_corpus` failures were re-run **alone at the
  untouched base** `32e2f57d` and fail identically there. Neither was claimed as this phase's green.

**Where confidence IS downgraded:** Criterion 2, because the acceptance log 23B-01 cites is not in
the tree (§C2). The criterion still stands on the retained 23B-02 re-drive, which has better
provenance — but a reader trusting `23B-01-LIVE-EVIDENCE.md`'s table would be trusting a run
nobody can re-read.

**Endorsed after independent re-derivation:** `BL-23B-H1`'s re-grade from HIGH to MEDIUM. Counted
by me from the raw logs in `evidence/23B-H1-measure/`, not from the write-up:

```
F23_H1_RUN= 92 · F23_H1_REACH= 92 · sum(tool_events) 153 · tool_events=0 runs: 0
status=OK 92 · status=CHECKSUM_MISMATCH 0 · file_written=no 0 · 3 distinct binaries
```

The harness is genuinely reach-proven — each `F23_H1_REACH` line carries a real `tool_events=`
count and `file_written=yes`, so the defect that made the *earlier* evidence worthless (dead port,
non-reaching runs folded into `resume_ok`) cannot recur. `scripts/f23-h1-repro-live.sh:112-117`
carries the **three-assertion self-test** — known-positive, known-negative, **and `_sum_old`
proving the old broken matcher would have missed it** — and lines 279-285 record an instrument
defect found *and repaired in the same lane*. That is the standard, and it is met here.
It remains a **non-reproduction, not a disproof**; root cause is unidentified.

---

## GAP LIST — costed, for the next execution wave

Lane-session = one dispatched lane's working session. "Credential" means it cannot be closed by
build work alone.

| ID | Crit | Missing capability (one line) | Lane-sessions | Needs |
|---|---|---|---|---|
| **G5a** | 5 | **Linux journey day script aborts on unbound `HOME` under systemd; day 3 fails in ~28h unless fixed** | **0.25** | pure build — **URGENT** |
| G5b | 5 | Linux days 2 and 3 must be re-run after G5a; real calendar time cannot be compressed | 0.5 + **2 calendar days** | pure build (elapsed-time floor) |
| G5c | 5 | macOS journey leg: CI does not upload the compiled test harness, and it cannot be built on the Mac | 1 | **`.github/workflows/ci.yml` edit — fenced from lanes; needs release-side authority** |
| G5d | 5 | `BudgetWallClockAuthority::AbsoluteDeadline` has zero production construction sites — no user can reach it | 1 | pure build |
| G3a | 3 | Outbound-provider-body forget proof: plant a value, assert present in captured body, forget, assert absent in the next | 1 | pure build (mock provider — **no credential**) |
| G3b | 3 | Surface the nudge bound as a user-reachable command | 0.5 | pure build |
| G3c | 3 | User-model correction precedence | 1 | pure build |
| G3d | 3 | Drive `/memory why\|correct\|forget\|privacy\|retention` live through a real TUI on ≥1 platform | 1 | pure build |
| G4a | 4 | Cache hit/invalidation reasons + token-pressure state promoted from log telemetry to a user surface | 1.5 | pure build |
| G4b | 4 | Compaction quality gates and cost-regression thresholds | 1.5 | pure build |
| G2a | 2 | `scripts/f23-session-operator-drive.ps1` does not exist — the Windows session leg is unbuilt | 1 | pure build |
| G2b | 2 | macOS session-operator leg (binary route already open via CI artifacts) | 0.5 | pure build |
| G2c | 2 | TUI `/checkpoint`, `/fork`, `/export` + one PTY leg | 1.5 | pure build |
| G2d | 2 | Repair path for an unreadable journal — today one mismatch takes all twelve `session` verbs down | 1.5 | pure build |
| G1a | 1 | Governed **promotion** (`bail!`, untouched) | 1-2 | pure build — **23A lane's** |
| G6a | 6 | OPTIONAL semantic/RRF layer — would close 23B-03-M1's three mis-ordered concept queries | 1.5 | pure build — **not required for the criterion** |

**Totals:** ~16.5 lane-sessions of pure build, **one item requiring authority this lane does not
have (G5c)**, and **a hard 2-calendar-day floor on G5b that no amount of parallelism removes.**
No gap needs a provider credential.

**Sequencing recommendation.** G5a first and today — it is 15 minutes of work and 28 hours from
costing another multi-day cycle. Then G5b in the background while G3a (the smallest step to a real
F23-03 disposition) and G4a/G4b proceed in parallel. G5c needs a decision from someone who may
touch `ci.yml`.

---

## What is ungradeable, and why

- **Criterion 5 cannot be graded MET by anyone before 2026-07-31T00:05Z at the very earliest** —
  and that date assumes G5a is fixed today and Windows day 3 lands as scheduled. A three-real-day
  span is a floor, not an estimate. It is graded NOT MET on present state, which is a statement
  about today and not a prediction.
- **Criterion 1's authority is the 23A lane's**, running concurrently. My PARTIAL is from in-tree
  evidence and should be superseded by theirs if they differ.
- **Root cause of 23B-H1 remains unidentified.** 92 reach-proven runs at zero is a
  non-reproduction; the original sighting's provider configuration was never recorded and cannot
  now be reconstructed. That residual is not closable by measurement and is correctly parked at
  MEDIUM.

---

## Fence report vs `861d1b1a`

```
git diff --name-status 861d1b1a HEAD
A  .planning/phases/23B-continuous-agency/23B-GRADE-NOTES.md
git status --porcelain    -> (empty)
git diff --stat 861d1b1a HEAD -- crates/ .github/workflows \
    crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs   -> (empty)
```

**Zero fence exposure.** No `crates/` change, no `.github/workflows/` change, no shared-file edit.
This VERDICT file and `23B-GRADE-NOTES.md` are the only additions. No merge, no PR, no tag, no
issue touched, no `wcore-contract generate`. Read-only ssh reads on `hetzner-dsm` and
`SeanD@seandesktop`; **no journey state was mutated on either host.**

---

_Graded 2026-07-29T10:03Z by lane `grade-23b`. Every number above was produced by an unproxied
tool (`/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/find`, `/usr/bin/wc`); raw measurements are in
`23B-GRADE-NOTES.md`._
