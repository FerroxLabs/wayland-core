---
issue: 1268
repo: FerroxLabs/wayland
kind: defect
title: "The #1248 notice path IS reachable on Windows: a structural-impossibility claim contradicts atomic_io.rs's own correction (split from #1248)"
status: open
last_verified_commit: 4f2ef0ae
criteria:
  - id: c1
    text: "The false sentence is corrected wherever it is committed — `write.rs`, `edit.rs`'s reference to it, and the `wayland-1248` ledger note — and the corrected text states the real reason the tests are gated (no Windows executor in this workspace), not a structural impossibility."
    state: met
    evidence: "file:.planning/ledger/wayland-1248.md:73:The claim recorded here was FALSE"
    owner: core
    note: "MET 2026-08-31, lane/f13-windows. The false sentence is corrected in all three places the issue names, and the correction is anchored so it cannot rot back. write.rs: the unix gate on `the_vfs_path_names_a_save_the_refusal_displaced` is GONE rather than re-worded, and its doc now quotes the historical claim only to name it false against `wcore_config::atomic_io`'s own correction; the two tests that stay unix-gated (`a_refusal_that_could_not_be_rolled_back_is_not_a_fallback`, `execute_reports_a_refusal_it_could_not_roll_back`) now say the reason is ROLLBACK FAILURE on the exchange platforms -- `ReplaceFileW` answers ERROR_FILE_NOT_FOUND against a vanished destination, `publish_displacing` maps it to `Swap::Vacant`, the plain-rename fallback succeeds, so no `RollbackFailed` is produced -- which is a statement about that state and NOT about what Windows can observe. edit.rs's reference went with its own gate. The `wayland-1248` ledger residual is rewritten: it still said `Both c3 tests are #[cfg(any(linux, macos))]`, which was stale the moment the gates came off, and it now carries the measurement instead. EVIDENCE IS THE LEDGER HALF ON PURPOSE: the two crate halves are the c4 guard's subject and redden it, so one anchor per criterion is not a gap. RED ARM, hetzner, against this tree: re-injecting the exact historical sentence into write.rs BESIDE its own correction -- the adjacency that defeated the guard's first version -- compiles (MUTATION_SITES=1, CHECK_EXIT=0) and reddens the guard (RED_TESTS_EXIT=100), which names `crates/wcore-tools/src/write.rs: Windows publishes with ReplaceFileW and restores with a plain replacing rename, which hands nothing back to judge, so no save can be intercepted there at all`. Restored blob 245508dad3f2631ce63cc9e6cf5b568ebd19a93d == HEAD blob, `git status --porcelain` empty, green control on the restored tree 1 passed."
  - id: c2
    text: "The `intercepted_save: Some(..)` path is exercised on Windows: either the two `the_vfs_*_path_names_a_save_the_refusal_displaced` tests are made to run on a Windows host, or a Windows-only test drives `atomic_write_checked` through a displaced save and asserts the surfaced text names the preserved file."
    state: met
    evidence: "test:crates/wcore-tools/src/write.rs::the_vfs_path_names_a_save_the_refusal_displaced"
    owner: core
    note: "MET 2026-08-31 on REAL WINDOWS -- the branch this criterion offers as its first option is the one taken: the two `the_vfs_*_path_names_a_save_the_refusal_displaced` tests are made to RUN on a Windows host, not replaced by a Windows-only substitute. The unix `#[cfg]` came off both (write.rs and edit.rs), and one production compile fix was needed to get there: `restore`'s `#[cfg(not(any(linux, macos)))]` arm still matched `Swap::Unsupported` without its payload, so that arm had not been compiled for a Windows target at all. RUN: SeanDesktop, Windows 10.0.26200.9168, D:\\w-f13\\win13 at 91940861 with `git status --porcelain` EMPTY, `cargo nextest run -p wcore-tools --retries 0 -E 'test(the_vfs_path_names_a_save_the_refusal_displaced) or test(the_vfs_edit_path_names_a_save_the_refusal_displaced)'` -> `2 tests run: 2 passed, 1701 skipped`, exit 0. NEGATIVE CONTROL in the same session, because an empty selection reads exactly like a pass: a deliberately non-existent test name gives `0 tests run: 0 passed, 1703 skipped`, exit 4. RED ARM, on the PRODUCTION path and not on the test: `restore`'s Windows arm changed to `Swap::Displaced(_) => Ok(None)` -- i.e. `ReplaceFileW`'s `lpBackupFileName` result thrown away, which is exactly what the false sentence claimed the platform did -- compiles (MUTATION_SITES=1, CHECK_EXIT=0) and gives `2 tests run: 0 passed, 2 failed`, RED_TESTS_EXIT=100. Restored blob ce3fc492e3331ccdb1abb2eb7b01106d13be6c99 == HEAD blob, tree clean. So `intercepted_save: Some(..)` is not merely reachable on Windows: it is REACHED, the surfaced refusal names the preserved file, and the assertion fails the moment the platform stops handing the pre-image back. || RE-VERIFIED INDEPENDENTLY 2026-08-31 on real Windows 10.0.26200.9168 (SeanDesktop), clean tree, --retries 0: the two tests are `2 tests run: 2 passed, 1701 skipped` (exit 0) and the negative control `test(this_test_name_does_not_exist_anywhere)` is `0 tests run` exit 4, so an empty selection cannot read as a pass. RED ARM RE-RUN ON THE PRODUCTION PATH by this lane rather than inherited: `restore`'s non-unix arm mutated to `Swap::Displaced(_) => Ok(None)` -- ReplaceFileW's `lpBackupFileName` result thrown away, which is what the false sentence claimed the platform did -- MUTATION_SITES=1, CHECK_EXIT=0, `2 tests run: 0 passed, 2 failed`. Restored blob ce3fc492e3331ccdb1abb2eb7b01106d13be6c99 == HEAD blob, tree clean. ALSO CORRECTED, because it is stated on the issue and is now STALE: the comment on wayland#1268 reports that `integ/f13` does not compile for Windows (E0532 in `atomic_io.rs`). That was true of 3847cb788; it is NOT true of the current integ tip 70a47aaed, which is an ANCESTOR of this branch -- `cargo check -p wcore-config --target x86_64-pc-windows-gnu` at 70a47aaed exits 0 (measured on hetzner, 2026-08-31), so that blocker no longer stands against `#350` c5."
  - id: c3
    text: "If c2 measures the path as *not* working on Windows, that is filed as its own defect with the measurement, rather than being absorbed back into a doc comment."
    state: met
    evidence: "test:crates/wcore-tools/src/edit.rs::the_vfs_edit_path_names_a_save_the_refusal_displaced"
    owner: core
    note: "MET 2026-08-31 BY A FALSE ANTECEDENT, and that is said plainly rather than left for `met` to imply a filing that never happened. This criterion is conditional -- \"IF c2 measures the path as NOT working on Windows, that is filed as its own defect with the measurement\". c2 measured it WORKING: 2 of 2 on real Windows 10.0.26200.9168 at --retries 0, with a negative control proving the filter selects and a production-path red arm proving the pass is not vacuous. There is therefore no defect to file, and nothing has been absorbed back into a doc comment -- the doc comments went the other way, from a structural-impossibility claim to a measurement, and c4's guard fails the build if that reverses. The evidence token is c2's own measurement, deliberately: this criterion's content is entirely about what that measurement showed, so anchoring it anywhere else would be anchoring it to something that cannot go stale with the fact it depends on. If a later change breaks the Windows path, that test reddens and this criterion's antecedent becomes true again with the measurement already in hand."
  - id: c4
    text: "A grep gate or test proves no other doc comment in `crates/wcore-tools` or `crates/wcore-config` asserts a Windows structural impossibility that `atomic_io.rs:442-451` contradicts."
    state: met
    evidence: "test:crates/wcore-config/tests/issue_1268_windows_impossibility_guard.rs::no_doc_comment_claims_the_displaced_save_path_is_impossible_on_windows"
    owner: core
    note: "MET 2026-08-31, lane/f13-windows. `crates/wcore-config/tests/issue_1268_windows_impossibility_guard.rs` sweeps every `//` comment under `crates/wcore-tools` and `crates/wcore-config` -- src and tests -- and fails on any SENTENCE that is about Windows or `ReplaceFile`, about the displaced-save subject `atomic_io.rs:442-451` governs, and asserts an impossibility, unless that sentence is itself correcting such a claim. Sentence granularity, not block: an earlier version graded blocks, and re-injecting the historical claim directly ABOVE its own correction did not redden it, because the two `//` runs joined and the correction's exemption covered the offence. That case is now a control inside the test. Three more controls run before the sweep -- the historical sentence must be flagged, the corrected sentence must not be, an ordinary unrelated Windows comment must not be -- and the sweep refuses a run that saw fewer than 20 files or 500 comment lines, so an empty offender list off an empty scan cannot read as a clean tree. The one exclusion is the guard file itself, and the count is asserted to be exactly 1 so a second exclusion cannot launder a real offender. RED ARM: see c1's note -- same mutation, CHECK_EXIT=0, RED_TESTS_EXIT=100, blob-verified restore. GREEN on real Windows 10.0.26200.9168 as well (1 test run, 1 passed), because a source guard that only ever ran on Linux would be the same class of gap this issue is about. || RE-VERIFIED 2026-08-31 on real Windows 10.0.26200.9168 (SeanDesktop): `1 test run: 1 passed`. RED ARM by this lane in the ADJACENCY shape the test claims to handle -- the exact historical sentence injected into `write.rs` immediately ABOVE its own correction, so the two `//` runs join: MUTATION_SITES=1, CHECK_EXIT=0, `1 test run: 0 passed, 1 failed`. Restored blob 245508dad3f2631ce63cc9e6cf5b568ebd19a93d == HEAD blob, tree clean."
---

Created 2026-08-31 to close a COVERAGE gap; GRADED the same day by lane
`lane/f13-windows`, and all four criteria are now met.

READY FOR MAINTAINER CLOSE, and the gate will say so LOUDLY until it happens:
`check-criteria-ledger.py` (online) reports DIVERGENCE for a ledger whose every
criterion is met while the issue is still open. That is the intended handoff
signal, not a defect in this file -- closing an issue is a maintainer action and
this lane does not take it. CI runs the gate with `--offline`, which does not
consult GitHub state, so no required check is red meanwhile.

## Found on the way in, and bigger than this issue: `integ/f13` DOES NOT BUILD
## FOR WINDOWS

Measured, not inferred. On `integ/f13` at `3847cb788`:

```
cargo check -p wcore-config --target x86_64-pc-windows-gnu
  error[E0532]: ... crates/wcore-config/src/atomic_io.rs:467
  Swap::Vacant | Swap::Unsupported => std::fs::rename(displaced, dest).map(|()| None),
  error: could not compile `wcore-config` (lib) due to 1 previous error
```

`git log -S "Swap::Unsupported("` names one commit: `727ea921f`, "Make the
Windows publish degrade observable and declare the weaker guarantee"
(2026-08-30). It gave `Swap::Unsupported` a payload and updated the
`cfg(windows)` sites, but not `restore`'s `cfg(not(any(linux, macos)))` arm --
the one arm a Linux `cargo check` cannot see. That is the shared-type change
shape: verify with `--workspace --all-targets` AND the Windows target, never
`-p` on Linux alone.

Consequences, stated because they are larger than one ledger row:

* `wcore-config` is at the bottom of the graph, so NOTHING in the workspace
  built for Windows between 2026-08-30 and this branch. Every Windows verdict
  taken from `integ/f13` in that window is a verdict about a build that does
  not exist, and `integ/f13` CI has been failing.
* `wayland-core#350` c5 (a green nightly-windows-soak) cannot be met on
  `integ/f13` at all; the soak fails at compile, not at the AppContainer race
  its ledger names as the sole remaining blocker.

The fix is this branch's `fix(atomic-io): compile the non-unix restore arm`,
which is also what made `#1268` c2 possible: the tests could not have been run
on Windows without it.


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
