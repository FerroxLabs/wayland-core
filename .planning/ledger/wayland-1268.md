---
issue: 1268
repo: FerroxLabs/wayland
kind: defect
title: "The #1248 notice path IS reachable on Windows: a structural-impossibility claim contradicts atomic_io.rs's own correction (split from #1248)"
status: open
last_verified_commit: b45f08119
criteria:
  - id: c1
    text: "The false sentence is corrected wherever it is committed — `write.rs`, `edit.rs`'s reference to it, and the `wayland-1248` ledger note — and the corrected text states the real reason the tests are gated (no Windows executor in this workspace), not a structural impossibility."
    state: met
    evidence: "file:crates/wcore-tools/src/edit.rs:696:the gate was an absent Windows executor"
    owner: core
    note: "MET. The false sentence is corrected at all three committed sites, checked by grep across the whole tree rather than by trusting the lane: `grep -rn 'hands nothing back to judge'` returns FIVE hits and every one of them is either a QUOTATION inside a correction or a FIXTURE inside the c4 guard -- crates/wcore-tools/src/write.rs:922 (the correction, which quotes the claim in order to name it false), .planning/ledger/wayland-1248.md:73 (the ledger note, rewritten under the heading `Windows -- CORRECTED 2026-08-30, the earlier claim here was FALSE`), and three inside crates/wcore-config/tests/issue_1268_windows_impossibility_guard.rs, which must contain it because it is the string the guard's own positive control asserts is flagged. There is no un-corrected instance left. THE CORRECTED TEXT STATES THE REAL REASON, which is the half of c1 that is easy to skip: write.rs:919 says the tests ran on Linux/macOS only `because this workspace had no Windows executor`, and edit.rs:696 says `the gate was an absent Windows executor, never an unreachable path` -- a workspace fact, not a structural impossibility. Both sentences are now ALSO true of the past rather than the present, because the executor was found and the tests have since run on it (see c2). Enforcement, so this cannot silently regress, is c4's guard."
  - id: c2
    text: "The `intercepted_save: Some(..)` path is exercised on Windows: either the two `the_vfs_*_path_names_a_save_the_refusal_displaced` tests are made to run on a Windows host, or a Windows-only test drives `atomic_write_checked` through a displaced save and asserts the surfaced text names the preserved file."
    state: met
    evidence: "test:crates/wcore-tools/src/write.rs::the_vfs_path_names_a_save_the_refusal_displaced"
    owner: core
    note: "MET -- EXERCISED ON REAL WINDOWS, which is the word c2 uses and which removing a `#[cfg]` does not satisfy on its own. Run https://github.com/FerroxLabs/wayland-core/actions/runs/33352985296, job `CI (Array)`, self-hosted runner `ferrox-win-msvc`, commit b45f08119 (tree byte-identical to lane/f13-windows @ 91940861e). c2's FIRST branch was taken: the two ungated tests were made to run on a Windows host. BOTH PASSED, by name, from the `nextest-junit-Array` artifact: `wcore-tools write::tests::the_vfs_path_names_a_save_the_refusal_displaced` PASS 0.230s and `wcore-tools edit::tests::the_vfs_edit_path_names_a_save_the_refusal_displaced` PASS 0.196s, neither carrying a `flaky-runs` attribute. COLLECTION IS THE LOAD-BEARING FACT HERE AND IT IS PROVED, NOT INFERRED. This is precisely the ticket where `0 tests ... ok` reads exactly like a pass, so the verdict is NOT taken from the job log -- that log is TRUNCATED (12,618 `PASS [` lines against 16,223 tests run) and the two names do not appear in it at all. It is taken from the JUnit artifact, which carries all 16,223 `<testcase>` elements and in which both tests are present and green. A pass means the path was TAKEN, not merely reached: each test drives the tool through `execute_with_ctx` over a real `RealFs`, makes a save from inside the exchange-to-verdict window, asserts the window was entered EXACTLY ONCE and with the pre-image, and asserts the surfaced `ToolResult` names the preserved file -- so `Refusal { intercepted_save: Some(..) }` was constructed on Windows through `ReplaceFileW`'s `lpBackupFileName`, which is exactly what the false sentence said could not happen. MEASURED ON A SECOND WINDOWS FLEET TOO: the same run's `CI (windows-latest, hosted)` leg on a GitHub-hosted `windows-latest` runner also has both tests PASS (0.351s / 0.358s), so the result is not a property of one box. LINUX CONTROL, so a Windows pass is not read as proving more than it does: both tests also pass on hetzner (`cargo test -p wcore-tools --lib the_vfs_` -> `2 passed`), so the Windows result is a second platform and not a first green. The unbacked sentence `MEASURED on real Windows rather than argued; see the ledger entry for wayland#1268` at write.rs:932 was a forward reference when it was written; this entry is the measurement it points at, and it is now true."
  - id: c3
    text: "If c2 measures the path as *not* working on Windows, that is filed as its own defect with the measurement, rather than being absorbed back into a doc comment."
    state: met
    evidence: "test:crates/wcore-tools/src/edit.rs::the_vfs_edit_path_names_a_save_the_refusal_displaced"
    owner: core
    note: "MET -- and met by its ANTECEDENT BEING MEASURED FALSE, not by the work being done, which is stated plainly here so nobody reads this as a discharged obligation. c3 fires only `if c2 measures the path as NOT working on Windows`. c2 measured it WORKING: both `the_vfs_*_path_names_a_save_the_refusal_displaced` tests PASS on real Windows in run https://github.com/FerroxLabs/wayland-core/actions/runs/33352985296, so there is no not-working measurement to file and nothing was absorbed back into a doc comment. THE SPIRIT OF c3 WAS ALSO HONOURED UNDER THE BROADER READING, because the same run did surface Windows bad news next door and it was NOT quietly absorbed: `wcore-tools::issue_1248_conflict_notice_test in_memory_backend_conflict_still_renders_todays_wording` FAILED on Windows -- same test family, same #1248 lineage -- with `Refused to write /w/f.txt: path must be absolute`, a POSIX absolute path in the fixture that is not absolute on Windows. That is a DIFFERENT arm from the one c2 names (the in-memory backend's conflict-that-displaced-nothing wording, not the `intercepted_save` displaced-save path), so it does not make c2's antecedent true; and it was FILED, with the verbatim assertion, as FerroxLabs/wayland-core#409 rather than written into a comment. If a later reader decides that failure DOES belong to c2's subject, the measurement and the carrier both already exist and this criterion can be re-graded off them without re-running anything."
  - id: c4
    text: "A grep gate or test proves no other doc comment in `crates/wcore-tools` or `crates/wcore-config` asserts a Windows structural impossibility that `atomic_io.rs:442-451` contradicts."
    state: met
    evidence: "test:crates/wcore-config/tests/issue_1268_windows_impossibility_guard.rs::no_doc_comment_claims_the_displaced_save_path_is_impossible_on_windows"
    owner: core
    note: "MET, and re-confirmed independently rather than inherited. The guard is crates/wcore-config/tests/issue_1268_windows_impossibility_guard.rs, and it is not platform-gated, so it grades on every leg. RUN ON LINUX on hetzner at this tree: `cargo test -p wcore-config --test issue_1268_windows_impossibility_guard` -> `running 1 test ... ok. 1 passed; 0 failed` -- ONE test collected, not zero, which is the check that matters for a gate whose failure mode is an empty scan. ALSO RUN ON REAL WINDOWS in https://github.com/FerroxLabs/wayland-core/actions/runs/33352985296: `wcore-config::issue_1268_windows_impossibility_guard no_doc_comment_claims_the_displaced_save_path_is_impossible_on_windows` PASS 0.535s in the JUnit artifact. It is non-vacuous by construction and the construction was read, not assumed: three controls run BEFORE the sweep (the historical false sentence MUST be flagged; the corrected sentence that replaced it MUST NOT be; an ordinary Windows comment on an unrelated subject MUST NOT be), and the sweep FAILS OUTRIGHT on fewer than 20 files or 500 comment lines, so an empty offender list off an empty scan cannot read as a clean tree. It also grades per SENTENCE rather than per comment block, after the block version was MEASURED to miss a false sentence sitting directly above the correction that exempted it."
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

## Graded on real Windows, 2026-08-31

This ledger was created two days ago recording that **no work was done**, and it still read
`not-met` across the board while most of the work was in fact finished. The blocker was never
the code. `c2` asks for the path to be **exercised on Windows**, and removing a `#[cfg]` is not
an execution: the ungated tests still had to reach a Windows host, and no commit on
`lane/f13-windows` carried the `[ci-windows]` marker that `ci.yml` requires (ci.yml:239, 284,
921) for a non-`main` branch to get one.

Closed by pushing a marker-carrying commit whose tree is byte-identical to the lane's and
reading the result:
[run 33352985296](https://github.com/FerroxLabs/wayland-core/actions/runs/33352985296),
`CI (Array)`, `ferrox-win-msvc`.

| criterion | instrument | verdict |
|---|---|---|
| c1 | grep of all three committed sites + the c4 guard | corrected everywhere; real reason stated |
| c2 | both `the_vfs_*` tests, on Windows | **PASS** 0.230s / 0.196s (and 0.351s / 0.358s on a hosted `windows-latest` runner in the same run) |
| c3 | c2's antecedent | measured **false** — nothing to file |
| c4 | the impossibility guard | **PASS** on Linux (1 collected) and on Windows |

The verdicts come from the run's `nextest-junit-Array` artifact, not from the job log. That
matters on this ticket specifically: the log is truncated at 12,618 `PASS` lines against 16,223
tests, neither `the_vfs_` name appears in it, and an absence there would have looked exactly
like the "0 tests" vacuity this issue exists to stop.

## Residual, named rather than absorbed

The Windows leg is red overall (`5 failed`), and one of the five is in this ticket's own test
family: `issue_1248_conflict_notice_test in_memory_backend_conflict_still_renders_todays_wording`
fails on Windows because the fixture uses a POSIX absolute path. It is a **different arm** from
the `intercepted_save` path `c2` governs, so it does not reopen `c2` — and it is filed with its
verbatim evidence as **FerroxLabs/wayland-core#409**, together with the other four, rather than
being written into a doc comment. That is the discipline `c3` exists to enforce.

All four criteria now hold. Closing the issue is Sean's.
