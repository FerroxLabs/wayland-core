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
