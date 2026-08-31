---
issue: 1268
repo: FerroxLabs/wayland
kind: defect
title: "The #1248 notice path IS reachable on Windows: a structural-impossibility claim contradicts atomic_io.rs's own correction (split from #1248)"
status: open
last_verified_commit: cfcf97d0
criteria:
  - id: c1
    text: "The false sentence is corrected wherever it is committed — `write.rs`, `edit.rs`'s reference to it, and the `wayland-1248` ledger note — and the corrected text states the real reason the tests are gated (no Windows executor in this workspace), not a structural impossibility."
    state: met
    evidence: "file:crates/wcore-tools/src/write.rs:919:NO LONGER GATED TO UNIX"
    owner: core
    note: "MET cfcf97d0, and NOT inherited -- re-verified against this branch's own tree. All three committed sites were checked. (1) write.rs: the structural claim is gone; the block at :919-949 states the gate was an absent Windows executor, quotes wcore_config::atomic_io's own correction ('was simply wrong about lpBackupFileName'), and ends by saying the Windows execution is recorded as not-met and that the comment must not be read as evidence it was run. (2) edit.rs:695-698 refers to the write.rs block for the same reason. (3) .planning/ledger/wayland-1248.md: THIS ONE WAS STILL WRONG AT THE BASE, in a NEW way, and this lane fixed it. Its 2026-08-30 correction replaced the impossibility with the real reason but stated the gate in the PRESENT tense -- 'Both c3 tests are #[cfg(any(target_os = linux, target_os = macos))]' -- and commit 7d4a7c928 had since ungated both tests, so a reader would go looking for a cfg that is not there. Verified in the tree, not assumed: no cfg(any(target_os=linux,target_os=macos)) attribute sits on either test (the two remaining ones in write.rs are at :741 and :817, on other tests), and the two tests carry only #[tokio::test]. The bullet now records all three versions -- the false one, the stale one, and what is true at this commit -- because deleting a wrong sentence without saying what replaced it is how the second wrong sentence got written. GREP WITH A CONTROL: 'hands nothing back' now occurs only inside corrections and inside the c4 guard's own control strings, and the known-positive control query ('simply wrong about') returns the atomic_io correction, so the first result is an absence and not a broken query. Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
  - id: c2
    text: "The `intercepted_save: Some(..)` path is exercised on Windows: either the two `the_vfs_*_path_names_a_save_the_refusal_displaced` tests are made to run on a Windows host, or a Windows-only test drives `atomic_write_checked` through a displaced save and asserts the surfaced text names the preserved file."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "If c2 measures the path as *not* working on Windows, that is filed as its own defect with the measurement, rather than being absorbed back into a doc comment."
    state: blocked
    owner: maintainer
    note: "BLOCKED cfcf97d0 by lane doc-truth, and reported as blocked rather than graded, because its antecedent is a measurement this lane cannot make. c3 is conditional on c2: it obliges a filing only IF c2 measures the intercepted-save path as NOT working on Windows. c2 needs a real Windows host; this lane has none and is Linux-only by assignment, so the antecedent's truth value is unknown here and c3 is neither met nor unmet on this branch. WHAT UNBLOCKS IT: c2's measurement. Whoever holds a Windows host grades c2 and then grades c3 in the same pass -- if the path works, c3 is met by FALSE ANTECEDENT and the note must say so in those words rather than implying a filing happened; if it does not work, c3 requires a new issue carrying the measurement, and explicitly NOT a sentence added back into a doc comment, which is the exact failure this ticket was split out to correct. NOT INHERITED FROM ANYWHERE: a Windows run reported on the issue by another lane is on a branch that is not an ancestor of this one, and grading a criterion on a measurement this branch does not carry is the failure mode the ledger exists to catch."
  - id: c4
    text: "A grep gate or test proves no other doc comment in `crates/wcore-tools` or `crates/wcore-config` asserts a Windows structural impossibility that `atomic_io.rs:442-451` contradicts."
    state: met
    evidence: "test:crates/wcore-config/tests/issue_1268_windows_impossibility_guard.rs::no_doc_comment_claims_the_displaced_save_path_is_impossible_on_windows"
    owner: core
    note: "MET cfcf97d0, re-verified by this lane against its own tree rather than inherited. The guard sweeps every .rs file under crates/wcore-tools and crates/wcore-config and grades per SENTENCE, not per comment block: it flags a sentence only when it is about Windows or ReplaceFile, about the displaced-save subject atomic_io.rs:442-451 governs, and asserts an impossibility -- and it exempts a sentence that is CORRECTING such a claim, since a correction must quote it. GREEN ARM: 1 test run, 1 passed, inside the 4970-test run below. RED ARM run by this lane, not read off a comment: the historical sentence ('Windows publishes with ReplaceFileW and restores with a plain replacing rename, which hands nothing back to judge, so no save can be intercepted there at all') re-injected as a doc comment into crates/wcore-tools/src/write.rs (MUTATION_SITES=1), cargo check CHECK_EXIT=0 first so the red is behaviour, then TESTS_EXIT=100 naming the offending file and the reconstructed sentence; restore blob-verified equal to the HEAD blob (5cf9b3a06d360d63c05172aeece1f3586557af23), tree clean. Its own anti-vacuity controls are intact and run before the sweep: the historical sentence must be flagged, the corrected sentence must NOT be, an ordinary unrelated Windows comment must NOT be, the adjacency case (offence written directly beside its own correction) must be flagged, and a run that walked fewer than 20 files or 500 comment lines fails outright -- an empty offender list off an empty scan reads exactly like a clean tree. Verified on hetzner at this commit, clean tree: cargo check --workspace --all-targets = 0; cargo clippy -p wcore-config -p wcore-sandbox -p wcore-cli --all-targets -- -D warnings = 0; cargo nextest run -p wcore-config -p wcore-sandbox -p wcore-cli --retries 0 = 4970 tests run, 4970 passed, 0 failed."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.

## Graded by lane doc-truth, cfcf97d0

c1 and c4 met on `lane/f13-s2-doc-truth` (base `ca15a48bf`). c2 untouched -- it needs a real
Windows host and belongs to the Windows lane. c3 BLOCKED on c2, for the reason its own note
gives.

The c1 and c4 ARTEFACTS were already in this lane's base: `7d4a7c928` ungated the two tests,
`0af8f2190` retracted the unmeasured claim in `write.rs`, and
`crates/wcore-config/tests/issue_1268_windows_impossibility_guard.rs` was already checked in.
What was NOT already true is the grade -- this ledger recorded all four criteria `not-met`
against a tree that already satisfied two of them -- and the `wayland-1248` ledger note,
which was still stale. This lane re-ran the evidence rather than inheriting it: the c4 guard
was reddened and restored on this branch, and every claim above was checked against this
worktree.
