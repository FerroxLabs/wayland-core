---
issue: 374
repo: FerroxLabs/wayland-core
kind: defect
title: "core#238 c6's evidence test cannot establish its premise on Windows: ENOTDIR provocation maps to NotFound, and it hard-fails the nightly soak"
status: closed
last_verified_commit: b52fb934
criteria:
  - id: c1
    text: "The test has a Windows arm that produces a genuinely non-NotFound fs::metadata failure"
    state: met
    evidence: "symbol:crates/wcore-tools/src/path_validation.rs::a_non_absence_stat_failure"
    owner: core
    note: "MEASURED ON SEANDESKTOP 2026-08-29 (Windows 11 build 10.0.26200.9168), tree D:\\u-win at b52fb934, `--retries 0`. GREEN: `Summary [   0.037s] 2 tests run: 2 passed, 4425 skipped` - the guard test and its wrong-refusal control `a_not_yet_created_write_target_is_still_allowed`, which must keep passing because a Windows arm that refused an ordinary not-yet-created Write target would be a far worse defect than the one being fixed. RED ARM, committed before mutating, and it does not invent a failure - it restores the ONE the issue reports. MUTATION, verbatim, and it lands on CODE (the `-` line is the block's tail expression, not a comment): `-            dir.join(\"L\".repeat(300))` / `+            let file = dir.join(\"not-a-dir.txt\");` / `+            std::fs::write(&file, b\"x\").expect(\"write\");` / `+            file.join(\"child.txt\")` - i.e. the Windows arm is put back to the Unix provocation, which is the pre-fix state. RED, verbatim: `assertion `left != right` failed: premise: this must be a NON-absence stat failure, got Os { code: 3, kind: NotFound, message: \"The system cannot find the path specified.\" } / left: NotFound / right: NotFound` at path_validation.rs:849, `Summary [   0.037s] 2 tests run: 1 passed, 1 failed, 4425 skipped`. That is byte-for-byte the failure #374 was filed on, so the instrument is measuring the reported defect and not a lookalike. THE CONTROL IS IN THE SAME INVOCATION: `a_not_yet_created_write_target_is_still_allowed` PASSED in the red run too, so the red is attributable to the provocation and not to a broken build. RESTORE VERIFIED: `git diff --stat` empty afterwards and the file mtime bumped, so cargo could not serve the mutated binary - the re-run recompiled (2m 21s) and gave `Summary [   0.037s] 2 tests run: 2 passed, 4425 skipped`. AND ON BOTH WINDOWS CI LEGS, run https://github.com/FerroxLabs/wayland-core/actions/runs/33266399460: `PASS [   0.027s] (14693/15988) wcore-tools path_validation::tests::a_path_whose_metadata_fails_for_a_reason_other_than_absence_is_refused` on `CI (windows-latest, hosted)`, and green on the self-hosted msvc leg whose only failure was an unrelated sandbox case. AND IN THE SOAK, which is where #374 was filed from: run 33266413002 PHASE G on windows-2025 reports `PASS [   0.041s] (3060/4126)` for it, at the exact index where the previous soak died on it 3 of 3 tries."
  - id: c2
    text: "The premise assertion is NOT weakened to make the test pass"
    state: met
    evidence: "test:crates/wcore-tools/src/path_validation.rs::a_path_whose_metadata_fails_for_a_reason_other_than_absence_is_refused"
    owner: core
    note: "The `assert_ne!(err.kind(), NotFound, \"premise: this must be a NON-absence stat failure\")` is byte-identical to what it was; only the PATH handed to it moved, into `a_non_absence_stat_failure`. That is the whole point - #374 says explicitly that a test which stops checking its premise is the vacuity this one was written to avoid, and the premise check is what turned a silent vacuous pass into a loud failure in the first place. The refusal assertion (`PathValidationError::Unstattable`) is unchanged too, so the test still grades the guard and not merely the provocation."
  - id: c3
    text: "The Unix ENOTDIR provocation is kept, so the arm that already graded still grades"
    state: met
    evidence: "symbol:crates/wcore-tools/src/path_validation.rs::a_non_absence_stat_failure"
    owner: core
    note: "`#[cfg(not(windows))]` still builds the file-as-a-directory-component path and writes the file first, exactly as before; only a `#[cfg(windows)]` sibling arm was added. Graded on hetzner Linux inside the full workspace CI-profile run: the test is among the 17343 passing, so the arm that was already working was not traded away for the new one."
  - id: c4
    text: "The chosen provocation is measured on a real Windows build, not taken from the issue's suggestions"
    state: met
    evidence: "symbol:crates/wcore-tools/src/path_validation.rs::a_non_absence_stat_failure"
    owner: core
    note: "MEASURED on SeanDesktop (Windows 11 build 10.0.26200.9168) with a standalone rustc probe BEFORE the test was written, because #374 offered three candidate provocations and TWO OF THEM DO NOT WORK on this build. Verbatim: `file-as-a-component ERR kind=NotFound raw=Some(3)`; `component>255 ERR kind=InvalidFilename raw=Some(123)`; `illegal char < ERR kind=InvalidFilename raw=Some(123)`; `392-char path (>MAX_PATH) ERR kind=NotFound raw=Some(3)`. So the over-long PATH the issue proposed is `NotFound` here and would have failed the premise exactly as the current provocation does; the over-long COMPONENT is not. The component spelling was preferred over the illegal-character one because every character in it is a legal filename character, so no earlier guard in `validate_user_path` - NullByte, UncPath, DeviceOrVerbatimPath, WindowsNullDevice, NotAbsolute, Traversal, SystemPath - can plausibly claim it before the metadata check. The table is recorded on the helper's doc comment, so the refuted suggestions cannot be re-tried blind."
---

The #238 c6 guard is real -- validate_user_path now refuses on every metadata
ErrorKind except NotFound -- but its test provoked the non-absence failure with
a file used as a directory component, which is ENOTDIR on Unix and
ERROR_PATH_NOT_FOUND on Windows. Rust reports that as NotFound, the one kind the
guard deliberately lets through, so the premise assertion fired and the test HARD
FAILED on the platform the guard exists for: 3 of 3 tries in nightly-windows-soak
run 33258858506 PHASE G, the only hard failure in 3060 tests.

The fix is a Windows arm, chosen by measurement rather than from the issue's
suggested list -- two of the three suggestions do not work on Windows 11 build
26200, including the one the issue named first.
