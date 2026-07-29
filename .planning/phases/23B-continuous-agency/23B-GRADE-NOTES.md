# 23B GRADE NOTES — running record (lane `grade-23b`)

Started 2026-07-29. Base `861d1b1a`. Worktree `.../lane-grade-23b`, branch `lane/grade-23b`.
Purpose: produce `23B-PHASE-VERDICT.md` — Phase 23's six Success Criteria graded against
evidence already in the tree. **Append after every measurement. Do not batch to the end.**

## Instrument discipline for this lane

- All load-bearing reads via `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/wc`, `/usr/bin/find`.
  `rtk` rewrites `git log`, `grep`, `cargo`, `wc -c` — measured, brief §3b.
- Any absence claim needs a known-positive in the same invocation (brief §3b-i).
- `cargo nextest` "flakiness" in this repo was fd exhaustion — 40 runs, 0 real failures.
  `.config/nextest.toml`'s `no-tests = "fail"` is silently ignored by the installed nextest,
  so a green suite may have run nothing. Downgrade confidence wherever evidence rests on either.
- Re-derive all arithmetic. Do not inherit a prior verdict's counts.

## The six criteria (from `.planning/ROADMAP.md` §Phase 23, lines 101-107)

1. Generated skills cannot execute before governed promotion; observable, revocable, rollback-able.
2. Users can search, inspect, checkpoint, retry, fork, rewind, export, retain, reconcile session effects.
3. Users can see/control memory/user-model activation, provenance, correction, forgetting, privacy,
   retention, nudges.
4. Cache and compaction expose quality, invalidation, token-pressure, cost truth.
5. Multi-day wait/resume/complete journey preserves cumulative authority, resource, evidence,
   memory, delivery state.
6. Persistent incremental hybrid repository index — bounded lexical/symbol/optional-semantic
   retrieval with provenance, staleness, privacy, performance truth.

Internal order (ROADMAP line 109): **23A owns criterion 1**; 23B owns 2-6. Criterion 1 is graded
here only from what is in-tree, and is flagged as a sibling lane's authority.

## Inventory taken (measured, not claimed)

`.planning/phases/23B-continuous-agency/` — 20 markdown files, 5036 lines; `evidence/` 39 files,
2467 lines. Four plans (23B-01..04), four SUMMARYs, three LIVE-EVIDENCE files, two phase
dispositions (rev1 + rev2), plus a five-file 23B-H1 record.

**No `23B-PHASE-VERDICT.md` exists.** Confirmed by directory listing above (the listing is the
known-positive: 20 other `.md` files were returned by the same `wc -l *.md` invocation).

## Prior claims to VERIFY, not inherit

- `23B-PHASE-DISPOSITION-v2.md` grades: C2 PARTIAL, C3 PARTIAL/NOT MET, C4 NOT MET,
  C5 NOT MET, C6 NOT MET. Its own header says the C6/C5 rows are **superseded** — 23B-03 and
  23B-04 have since executed. So rev2's body is stale on exactly the two criteria it calls
  unexecuted. Grade from the SUMMARYs + evidence logs, not from rev2.
- `F23-04` (criterion 4) reportedly NEVER STARTED — criterion 4 depends on it entirely.
- `23B-04` reportedly day-one-only; macOS nothing run; blocked until 2026-07-30T23:54:26Z.
- `BL-23B-H1` re-graded MEDIUM after 92 runs / 153 tool events / 0 mismatches. Its **earlier**
  evidence was worthless in both directions (dead port + placeholder key ⇒ no tool event ever
  dispatched; non-reaching runs counted as successes). Treat the re-grade's instrument with the
  same suspicion applied to the original.

## Open questions at this point

- Does 23B-03's "all three platform legs PASS" survive reading the actual drive logs, given the
  macOS-cargo-on-Mac prohibition that rev2 says blocked every macOS row?
- Is there ANY F23-04 artifact (cache/compaction cost truth) in `crates/`?
- Does the 23B-H1 re-grade harness have a known-positive proving it dispatches tool events?

_(Appended below as measurements land.)_

---

## MEASUREMENT LOG

### M1 — artifact existence (one invocation, 18 PRESENT / 4 ABSENT — instrument proven alive)

PRESENT with line counts: `session_lifecycle.rs` 1297, `session_cmd.rs` 499,
`session_operator_lifecycle.rs` 534, `f23-session-operator-drive.sh` 356,
`provenance.rs` 989, `v6_recall_control.sql` 35, repomap `store.rs` 1005 / `scope.rs` 404 /
`search.rs` 543, `index_cmd.rs` 201, `incremental_index.rs` 675, `retrieval_quality.rs` 386,
`f23-index-drive.sh` 514, `f23-index-drive.ps1` 473, `multi_day_journey_test.rs` 1018,
`f23-clock-probe.sh` 296, `f23-multi-day-journey.sh` 236 / `.ps1` 204.

ABSENT: `scripts/f23-macos-binary.sh`, `scripts/f23-context-economics-drive.sh`,
`crates/wcore-cli/tests/memory_control_lifecycle.rs`,
`crates/wcore-agent/tests/context_economics_test.rs`.

**The ABSENT set matches exactly what the SUMMARYs said was not built.** No summary claimed a
file that is missing. That is a point in favour of the narratives' honesty — measured, not assumed.

### M2 — F23-04 (criterion 4) has no artifact. Absence measured with a live instrument.

- `/usr/bin/grep -rln "F23_04\|F23-04" crates/ scripts/` → 5 files, and **all five are plan
  23B-04's multi-day-journey files** (plan number collision: plan `23B-04` implements requirement
  **F23-05**, not F23-04). Zero cache/compaction artifacts.
- Concept search (quoted globs, per §3b-i): `invalidation_cause|InvalidationCause|token_pressure|
  TokenPressure|cost_reconcil|CostReconcil|compaction_quality|CompactionQuality` over
  `crates/ --include=*.rs` → **20 hits, all in `crates/wcore-providers/src/cache_observation.rs`
  (19) + `lib.rs` (1)**. Known-positive in the same flags: `CacheDiagnostics` → 0, so the type is
  named otherwise; the 20-hit result is itself the proof the instrument was alive.
  **First attempt returned 0 because zsh ate the unquoted `--include=*.rs` — the exact trap
  §3b-i names. The 0 was discarded, not reported.**
- `cache_observation.rs` (278 lines) last touched by `38736654` (PR #186, cache-health
  **telemetry**) and `da5a18b5`; `cache_diagnostics.rs` last touched `58e64fc6` (2026-07-15,
  pre-23B); `compact/state.rs` last touched `2c70b7b8` (PR #65). Known-positive on the same
  `git log --` instrument: `provenance.rs` correctly shows the three 23B-02 commits.
- Only **2** references to `cache_observation`/`TokenPressure` across `wcore-agent` + `wcore-cli`.

**Conclusion (C4): no work exists. Criterion 4 is NOT MET, and the honest cause is that plan
23B-02 Task 2 was never started — which its own SUMMARY states.**

### M3 — index legs (C6) ran at TWO different SHAs. Re-derived, and it is benign.

- Linux `F23_03_PROVENANCE=ok sha=b33827d3`, macOS + Windows `sha=1eb2d7c2`.
- `git merge-base --is-ancestor b33827d3 1eb2d7c2` → YES.
- `git diff --stat b33827d3 1eb2d7c2 -- crates/wcore-repomap crates/wcore-cli/src/index_cmd.rs
  scripts/f23-index-drive.{sh,ps1}` → **product code IDENTICAL**; only `f23-index-drive.ps1`
  (+473, new) and `f23-index-drive.sh` (11 lines).
- The 11-line `.sh` delta is the **field-extractor repair** (anchored `sed` returned EMPTY for the
  FIRST field on a line). **So the Linux leg ran the broken extractor** — its log shows
  `F23_03_VERIFY=agrees= exit=6` (empty), while macOS/Windows show `agrees=false`.
- **Does that vacate a Linux gate? No — checked, not assumed.** `field verify agrees` is used at
  `f23-index-drive.sh:460` in an `echo` only; the verify gate at :461 reads `VERIFY_RC`. Every
  *asserted* extraction (`records`, `symbols`, `read`, `extracted`, mutation fields at :378-383,
  `search fallback` at :308) returned non-empty real values on Linux, so none was first-on-line.
  Cost of the defect: one informational field. **Confidence downgrade: minor, not material.**
- Platform identity is corroborated by paths that cannot be cross-faked: macOS
  `/private/var/folders/8h/…`, Windows `//?/C:/Users/seand/…`, Linux `/root/wayland-23B-03`.
- Secret-isolation is a known-negative (`STORE_NONCE_OCCURRENCES=0`) **and carries its own
  known-positive in the same run** (`STORE_CONTROL_OCCURRENCES=1`). This is the discipline §3b-i
  demands, and 23B-03 is the only plan in the phase that supplied it unprompted.
