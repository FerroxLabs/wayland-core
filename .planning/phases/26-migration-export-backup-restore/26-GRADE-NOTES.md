# Phase 26 — grading notes (lane `grade-26`, live, append-only)

Purpose: produce `26-PHASE-VERDICT.md`, which has never been written. This file is
committed early and re-committed after every measurement, per LANE-BRIEF §6b-i.

Base: `861d1b1a`. Branch `lane/grade-26`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-grade-26`.

## The four criteria (verbatim, `.planning/ROADMAP.md:139-143`)

1. Hermes/OpenClaw discovery and dry-run are typed, deterministic, secret-redacted, and non-mutating.
2. Selective import/export preserves provenance and quarantines executable content.
3. Backup, restore, profile migration, and reciprocal portability survive interruption and restore exact pre-operation state on rollback.
4. Hostile fixture corpora prove conflict, secret-source remapping, isolation, and recovery semantics.

## What exists to grade (inventory, minute 5)

- `26-01`..`26-04` PLAN+SUMMARY, `26-01-BASELINE.md`, `26-04-CERTIFICATION.md`, `26-GAPS-SUMMARY.md`.
- No `26-PHASE-VERDICT.md`. Confirmed absent.
- `26-04-CERTIFICATION.md` self-grades: SC1 CLOSED, SC2 CLOSED, SC3 OPEN, SC4 CLOSED;
  F26-01/02/04/05 CLOSED, F26-03 OPEN (first clause: F23 envelope → portable session corpus, unstarted).
- `26-GAPS-SUMMARY.md` claims SC3's interruption clause was then attacked and produced
  **F26-GAPS-H1 HIGH** — `QuarantineStore::save_index` truncating `fs::write` on the LIVE index
  once per admitted item; kill mid-window ⇒ 143,360-byte partial JSON, `migrate quarantined`
  exit 1, re-run refuses all 440 items. Claims fixed + re-proved. **Must verify the fix in
  source AND that the re-proof is a kill distribution, not inspection.**

## Claims I must NOT inherit

- [ ] C1: macOS real-install run (7 real secrets, 0 hits, homes unmutated) — and the
      certification's own admission that it ran at ancestor `b671f9ad`, NOT the certified tree.
- [ ] C2: "0 secret hits" is a **known-negative assertion** (§3b-i) — needs a planted-secret
      positive control in the SAME invocation or it is self-passing.
- [ ] C3: F26-GAPS-H1 fix present in `crates/` at HEAD + kill distribution re-proof.
- [ ] C4: **The import half.** PORT-* ledger says both peers migrate from each other and Core
      has no reciprocal path; import untouched. SC2 says "Selective import/export". Determine
      whether `migrate` actually WRITES into a Wayland home, or only plans + quarantines.
- [ ] C5: F26-03 first clause (F23 envelope) — certification says unstarted; re-derive.
- [ ] C6: nextest `no-tests = "fail"` silently ignored; EMFILE mis-read as flakiness.
      Downgrade confidence wherever a green suite is the only evidence.

## Measurements (appended as taken)

(none yet — minute 12)
