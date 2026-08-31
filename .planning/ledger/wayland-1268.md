---
issue: 1268
repo: FerroxLabs/wayland
kind: defect
title: "The #1248 notice path IS reachable on Windows: a structural-impossibility claim contradicts atomic_io.rs's own correction (split from #1248)"
status: open
last_verified_commit: ca15a48bf
criteria:
  - id: c1
    text: "The false sentence is corrected wherever it is committed — `write.rs`, `edit.rs`'s reference to it, and the `wayland-1248` ledger note — and the corrected text states the real reason the tests are gated (no Windows executor in this workspace), not a structural impossibility."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "The `intercepted_save: Some(..)` path is exercised on Windows: either the two `the_vfs_*_path_names_a_save_the_refusal_displaced` tests are made to run on a Windows host, or a Windows-only test drives `atomic_write_checked` through a displaced save and asserts the surfaced text names the preserved file."
    state: met
    evidence: "test:crates/wcore-tools/src/write.rs::the_vfs_path_names_a_save_the_refusal_displaced"
    owner: core
    note: "MET on real Windows by lane/f13-s2-win-proc, by the FIRST branch the criterion offers: the two `the_vfs_*_path_names_a_save_the_refusal_displaced` tests are made to RUN on a Windows host, not replaced by a Windows-only substitute. Measured on real Windows 10.0.26200.9168 (SeanDesktop), x86_64-pc-windows-msvc, cargo 1.95.0 / nextest 0.9.138, isolated checkout D:\\\\s2winproc at ca15a48bf with a CLEAN tree (the run printed COMMIT and TREE=[] before testing), --retries 0 so nothing is laundered. A cross-compiled --target x86_64-pc-windows-gnu check compiles these arms and does not execute them, so it is not the evidence of record. NEGATIVE CONTROL in the same session: -E 'test(this_test_name_does_not_exist_anywhere)' -> 0 tests run, exit 4 -- an empty selection cannot read as a pass here. HOST LIMIT, stated because it matters where privilege does: this box has Developer Mode ON, so it is not representative of an ordinary Windows host for anything privilege-dependent. Nothing in this row is privilege-dependent: it is process creation flags, a Job Object, and file I/O under the calling user. GREEN: `-E 'test(the_vfs_path_names_a_save_the_refusal_displaced) or test(the_vfs_edit_path_names_a_save_the_refusal_displaced)'` -> 2 tests run: 2 passed, 1717 skipped, exit 0. So on Windows `publish_displacing` returns `Swap::Displaced(backup)`, `restore` returns `Ok(Some(exchanged_out))`, `holds_exactly` is consulted, and `Refusal { intercepted_save: Some(kept) }` is not merely reachable but REACHED, with the surfaced text naming the preserved file. That settles c3 by FALSE ANTECEDENT: c3 asks for a defect filing only if c2 measures the path NOT working, and it works, so there is nothing to file -- this lane did not quietly absorb a failure back into a doc comment. RED ARM ON THIS TREE, on the PRODUCTION path and not the test: `restore`'s non-unix arm changed to `Swap::Displaced(_) => Ok(None)`, i.e. ReplaceFileW's `lpBackupFileName` result thrown away, which is exactly what the false sentence claimed the platform did. CHECK_EXIT=0, then `2 tests run: 0 passed, 2 failed`, RED_1268_EXIT=100, panicking at write.rs:1024 and edit.rs:763. MUTATION_SITES=2, NOT 1, AND THAT IS STATED RATHER THAN ROUNDED DOWN: the same line occurs in BOTH cfg-exclusive arms of `restore` (atomic_io.rs:439 under cfg(any(linux, macos)), and atomic_io.rs:466 under cfg(not(any(linux, macos)))). Only :466 compiles on Windows, so exactly one site was LIVE on the host under test and the other is not compiled there at all; the textual count is 2 and the behavioural count is 1. RESTORED_BLOB=ce3fc492e3331ccdb1abb2eb7b01106d13be6c99 == HEAD blob, DIRTY=[], mtime touched after mutation and after restore. Post-restore GREEN control: 2 tests run, 2 passed, exit 0. NEGATIVE CONTROL for the selection itself, in the same session: `-E 'test(this_test_name_does_not_exist_anywhere)'` -> 0 tests run, exit 4 -- an empty selection cannot read here as a pass."
  - id: c3
    text: "If c2 measures the path as *not* working on Windows, that is filed as its own defect with the measurement, rather than being absorbed back into a doc comment."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c4
    text: "A grep gate or test proves no other doc comment in `crates/wcore-tools` or `crates/wcore-config` asserts a Windows structural impossibility that `atomic_io.rs:442-451` contradicts."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
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
