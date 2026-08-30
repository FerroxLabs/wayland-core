---
issue: 238
repo: FerroxLabs/wayland-core
kind: defect
title: "Windows reserved DOS device names (NUL/CON/COMn) bypass path guard"
status: closed
last_verified_commit: f0060a2e8
criteria:
  - id: c1
    text: "A Write or Edit to a path whose final component is a Windows device does not report success while discarding the bytes"
    state: met
    evidence: "test:crates/wcore-tools/src/path_validation.rs::the_null_device_guard_fires_on_windows_and_only_on_windows"
    owner: core
    note: "fcc152bf. New PathValidationError::WindowsNullDevice; validate_user_path:162 calls is_windows_null_device_path before the is_absolute gate. WIRING CHECKED, not assumed: Write calls validate_user_path at write.rs:95 and :353, Edit at edit.rs:196 and :445 - both entry points of each. Platform is a parameter, so the refuse-on-Windows and allow-on-Unix arms both run on every host."
  - id: c2
    text: "The guard refuses only names that measurement shows are still devices on the build under test"
    state: met
    evidence: "test:crates/wcore-tools/src/path_validation.rs::only_nul_is_treated_as_a_windows_device_name"
    owner: core
    note: "The predicate is trim_end_matches(['.',' ']).eq_ignore_ascii_case('nul') and nothing else. CON, AUX, PRN, COM1, LPT1, aux.txt, NUL.txt, nul.log and con.json are all asserted NOT refused. The Win 11 26200 measurement table is recorded on the predicate doc comment at :73-87 with both controls named, so the refuted textbook blocklist cannot be re-filed blind."
  - id: c3
    text: "Wrong-refusal controls pin that aux.txt, NUL.txt, COM1, con.json and nul.rs still validate Ok"
    state: met
    evidence: "test:crates/wcore-tools/src/path_validation.rs::a_windows_nul_path_is_refused_end_to_end"
    owner: core
    note: "This is the arm that runs the controls through validate_user_path and requires Ok - CON, COM1, NUL.txt and aux.log are each written and re-validated. TWO CAVEATS: it is cfg(windows) so it is graded by CI's Windows job alone and is unrun against this tree; and of the five spellings the criterion names, con.json and nul.rs appear only in the all-host string predicate (as con.json and nul.log), so nul.rs is nowhere verbatim."
  - id: c4
    text: "A Windows probe records whether bare NUL is a device on the build under test and whether fs::metadata reports is_file() true for it"
    state: met
    evidence: "test:crates/wcore-tools/tests/windows_nul_device_probe.rs::the_bare_nul_device_is_measured_on_the_build_under_test"
    owner: core
    note: "RUN AND RECORDED 2026-08-29 on SEANDESKTOP over OpenSSH, from this branch at ab6b602f. The previous entry said `blocked / owner: maintainer` on the belief that the Windows box is Sean-only infrastructure; it is not, it is reachable, so the entry was wrong and not merely stale. Command: `cargo nextest run -p wcore-tools --no-capture -E \"binary(windows_nul_device_probe)\"` -> `PASS [   0.374s] (1/1)`, `1 test run: 1 passed, 0 skipped`. RECORD, verbatim: os build = `Microsoft Windows [Version 10.0.26200.9168]`; control ordinary.txt kind = `DiskFile`, is_file = `Some(true)` (the liveness control fired, so the probe is not calling everything a device); `<dir>\\NUL` kernel kind = `CharDevice`, metadata is_file = `None`; bare `NUL` kernel kind = `CharDevice`, metadata is_file = `None`; write to `<dir>\\NUL` reported `Ok`; read back from `<dir>\\NUL` = `Ok(0)`. WHAT THE NUMBERS SETTLE: bare NUL IS still a device on 26200 asked of the KERNEL (GetFileType = FILE_TYPE_CHAR) and not merely of the name, so the narrow guard is refusing a device and not an addressable user file - the data-destroying direction is ruled out. And `metadata is_file` is `None`, i.e. `fs::metadata` ERRORS on the device rather than reporting a regular file: the NonRegularFile arm is structurally blind here and the name guard is the only thing standing, which is exactly why c1 exists. The write/read-back half reproduces the reported defect verbatim - the write reports `Ok` and the read back returns 0 bytes. Residual unchanged and still knowingly open: build 20348 (Server 2019/2022) was NOT measured, no such host is reachable; it is documented on the predicate, not silently assumed. CORE OWNERSHIP CONFIRMED INDEPENDENTLY, from the workflow rather than from the run: the probe is `#![cfg(windows)]` with ZERO `#[ignore]` attributes (control: 14 of them in crates/wcore-sandbox/tests/live_fs_acl.rs), the ci.yml matrix carries a Windows/msvc leg, and its test step runs `vx just test-ci` -> `cargo nextest run --workspace` (justfile:68-70). So the probe was already EXECUTING and asserting on every Windows CI run before this entry was graded -- the verdict arm was always bought, and only the printed RECORD was missing, because nextest captures a passing test's stdout. That is what the `--no-capture` run above supplies. Nothing here needs Sean-only infrastructure, which is why this criterion is owned by core and not by the maintainer. MADE DURABLE 2026-08-29 on lane/f13-u-win-native, because the record above is a HAND run and the criterion is about the build UNDER TEST, which moves: both Windows legs of ci.yml -- the self-hosted msvc leg of `CI (Array)` (line 617) and `CI (windows-latest, hosted)` (line 1058) -- now carry a `Record the Windows NUL-device measurement (core#238 c4)` step that re-runs `binary(windows_nul_device_probe)` with `--no-capture` and appends the RECORD block to the job log and `$GITHUB_STEP_SUMMARY`. It is a RECORDER, not a gate, and the distinction is deliberate: the VERDICT arm is still `vx just test-ci`, which already runs and asserts this probe on every Windows leg. The step is `pwsh`, not `bash`, because every other `vx` invocation on a Windows leg in that file runs under pwsh and none runs under bash -- a record step that dies on a PATH difference records nothing. `if: !cancelled()` so the numbers still land when some other test reddened the leg, which is when they are most wanted. That closes the gap the previous note named as the only thing missing (the numbers, not the verdict) for every future Windows build rather than for this one."
  - id: c5
    text: "The scope is decided - narrow to bare NUL, or close as wont-fix with the 26200 measurement recorded"
    state: met
    evidence: "symbol:crates/wcore-tools/src/path_validation.rs::is_windows_null_device_name"
    owner: maintainer
    note: "TAKEN 2026-08-29 as Q5 in .planning/DECISIONS.md - build the narrow guard, bare NUL only - and fcc152bf shipped it with the Win 11 26200 measurement table recorded on the predicate and the residual (Server 2019/2022, build 20348) documented as knowingly open. Nothing is parked on the maintainer any more."
  - id: c6
    text: "The NonRegularFile guard no longer disappears when fs::metadata errors for any reason other than absence"
    state: met
    evidence: "test:crates/wcore-tools/src/path_validation.rs::a_path_whose_metadata_fails_for_a_reason_other_than_absence_is_refused"
    owner: core
    note: "Added 2026-08-29; this is the issue's second named mechanism and arguably the more serious half - a guard that vanishes exactly when the OS refuses to describe the target. Fixed at path_validation.rs:228: the if-let-Ok is now a match where NotFound alone still passes and every other ErrorKind returns a new Unstattable variant. The test asserts the PREMISE (a non-NotFound stat failure) before asserting the refusal, and a_not_yet_created_write_target_is_still_allowed is the control. STILL MET, BUT ITS EVIDENCE IS INERT ON WINDOWS - recorded 2026-08-29 rather than left to be rediscovered. The guard change is real and is graded on Unix, but the test's ENOTDIR provocation (a file used as a directory component) maps to ERROR_PATH_NOT_FOUND on Windows, which Rust reports as the one ErrorKind the guard deliberately lets through, so the premise assertion fires and the test HARD-FAILS there - 3 of 3 tries in nightly-windows-soak run 33258858506 PHASE G, the only hard failure in 3060 tests, and reproduced on a Windows 11 build 26200 workstation at retries=0: `assertion `left != right` failed: premise: this must be a NON-absence stat failure, got Os { code: 3, kind: NotFound, message: \"The system cannot find the path specified.\" }`. Filed as FerroxLabs/wayland-core#374, which asks for a Windows arm producing a genuinely non-NotFound stat failure and explicitly forbids weakening the premise assertion to make it pass. #374 IS CLOSED ON lane/f13-u-win-native 2026-08-29, so this evidence is no longer inert on Windows: the provocation is now chosen per platform by `a_non_absence_stat_failure`, and the premise assertion is untouched. The Windows provocation was MEASURED on 26200, not taken from #374 own suggestion list, and two of the three it suggested do not work there: a file-as-a-directory-component is `kind=NotFound raw=Some(3)`, a 392-character path (over MAX_PATH) is ALSO `kind=NotFound raw=Some(3)`, while a single COMPONENT of 300 characters is `kind=InvalidFilename raw=Some(123)` and an illegal character is the same. So the over-long PATH #374 proposed would not have established the premise either. Graded on SeanDesktop: PASS, with `a_not_yet_created_write_target_is_still_allowed` passing beside it as the wrong-refusal control."
---

The issue claims Windows reserved DOS device names slip past the user-path
guard in wcore-tools. The mechanism is real: validate_user_path has no
reserved-name check of any kind, and it is live, not legacy - both Read entry
points, render, jsonl, image inspect, email parse, tts, tool-result storage and
the browser tool all call it.

The filed fix is wrong. A measurement on Windows 11 build 26200 found that only
a bare NUL is still a device; CON, AUX, PRN, COM1, LPT1 and every
extension-bearing spelling behave as ordinary files. Shipping the textbook list
would refuse addressable user files - and this guard grades paths to the user's
own data, where a false refusal loses something.

So the residual is narrow: a Write to a bare NUL discards bytes and claims
success. Criteria come from the cluster A verification note of 2026-08-29.

The last sentence of that note said the scope question is parked with the
maintainer. It is not, any more: c5 was TAKEN as Q5 in .planning/DECISIONS.md
and shipped in fcc152bf, and c4 was RE-GRADED 2026-08-29 from
`blocked owner=maintainer` to `not-met owner=core`. The probe is not #[ignore],
the ci.yml Array matrix carries a self-hosted Windows/X64/msvc leg, and its
test step runs `vx just test-ci`, which on Windows is a `--workspace` nextest.
The probe therefore already executes and asserts on every Windows CI run; what
is missing is only the printed RECORD, because nextest captures stdout for a
passing test. That is a core-owned workflow change, not Sean-only infra.
