# Phase 25 — GRADE NOTES (running log, lane `grade-25`)

Started 2026-07-29. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-grade-25`,
branch `lane/grade-25`, base `861d1b1a716240165209336b1fa38d36f9445716` (verified with
`/usr/bin/git rev-parse`).

**Mandate:** Phase 25 has NO verdict file. Produce `25-PHASE-VERDICT.md` grading all four
ROADMAP Success Criteria. Verify existing evidence rather than inherit it. Re-derive all
arithmetic. Grading only — no `crates/`, no workflows, no build.

---

## Success Criteria (verbatim from `.planning/ROADMAP.md` lines 124-136)

**Goal:** Operators can run governed work across reference backends/nodes and manage plugins
through a complete, recoverable lifecycle.

1. The same task runs locally, in a container, over SSH, and on one hibernating cloud backend
   with equivalent policy, receipts, cancellation, and cleanup.
2. Nodes pair, advertise capability, revoke, recover offline, and handle mixed versions
   without losing authority attribution.
3. Plugins can be scaffolded, tested, signed, installed, approved, inspected, updated,
   rolled back, removed, published, and recovered.
4. Compromised keys/plugins/backends and denied secret/egress paths fail closed with no
   orphaned execution.

Requirements listed: F25-01..F25-05. **Note: ROADMAP lists FIVE requirements (F25-05
included) but only FOUR Success Criteria and four plans (25-01..25-04). F25-05 has no
criterion and no plan — flagged for the verdict as a scope question.**

---

## Prior claims to verify (NOT inherit)

`25-PHASE-STATUS.md` header table claims **all four MET**:
- C1 MET (lane/25-cloud 2026-07-28)
- C2 "MET on every named property, one limitation" (lane/25-hosts)
- C3 MET on Linux, PARTIAL on Windows
- C4 MET (lane/25-hosts)

But the SAME file's "graded verbatim" section (written 2026-07-27, partially superseded)
says **C2 NOT MET** and **C4 NOT MET**. The file itself acknowledges the header "used to
claim two of four MET while the verbatim gradings showed only Criterion 3."

The competitive ledger reportedly records `REACH-*` SOURCE -> REACHED and calls it "the only
family carrying a MET Success Criterion" — i.e. **exactly one MET**. That is in direct
conflict with the status file's four-MET table. **Resolving this conflict is the core of this
lane's job.** Three mutually inconsistent records exist:
  (a) verbatim 2026-07-27 gradings: 1 MET (C3, and only "on Linux")
  (b) status header table 2026-07-28: 4 MET
  (c) competitive ledger: exactly 1 MET

## Instrument warnings in force

- nextest "flakiness" here = fd exhaustion; 40 runs, 0 real failures. Any red `exec failed`
  is NOT a regression.
- `.config/nextest.toml` `no-tests = "fail"` is SILENTLY IGNORED by installed nextest ->
  a green suite may have run zero tests. Downgrade confidence wherever a criterion rests on it.
- A known-negative assertion (orphan count == 0, "no fallback", "no leak") is SELF-PASSING
  on a dead instrument. Every zero in this phase's evidence needs a known-positive in the
  SAME invocation. Phase 25's own history contains exactly this defect twice (finding #9,
  Windows MEASURED ZERO while orphan ran; and the cloud nonce-filter structural false zero).
- `rtk` rewrites `git log` / `grep` / `cargo` / `wc -c`. All load-bearing reads via
  `/usr/bin/`.

## Evidence inventory (present on disk, byte counts pending re-derivation)

Phase dir has: 25-01..25-04 PLAN/SUMMARY, plus 25-01-CLOUD-BACKEND-DECISION,
25-01-EQUIVALENCE-EVIDENCE, 25-02-CLI-GATE-DECISION, 25-02-LIFECYCLE-TRANSCRIPT,
25-03-NODE-EVIDENCE, 25-04-FAIL-CLOSED-EVIDENCE, 25-CLOUD-SUMMARY, 25-HOSTS-SUMMARY,
25-MACOS, 25-PHASE-STATUS. `evidence/` holds ~100 capture files plus subdirs
`25-01/`, `25-cloud/`, `25-macos/`.

## Working log

- [t0] Worktree + branch verified. ROADMAP criteria extracted verbatim. Conflict between
  three records identified. NOTES committed.
- [next] Read 25-01/25-CLOUD-SUMMARY + equivalence + cloud ledger -> grade C1, with
  specific attention to whether the SSH leg was ever run at the SAME commit as the other
  three (status file itself qualifies this as "a composition across two commits").
