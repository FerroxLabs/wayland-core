---
issue: 238
repo: FerroxLabs/wayland-core
kind: defect
title: "Windows reserved DOS device names (NUL/CON/COMn) bypass path guard"
status: open
last_verified_commit: 43848f75
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
    state: blocked
    owner: maintainer
    note: "PROBE AUTHORED 2026-08-29, RUN OWED - needs-platform-run. crates/wcore-tools/tests/windows_nul_device_probe.rs::the_bare_nul_device_is_measured_on_the_build_under_test asks the KERNEL (GetFileType on a live handle, FILE_TYPE_CHAR vs FILE_TYPE_DISK) rather than the name, records BOTH facts the criterion names - is bare NUL a device, and does fs::metadata report is_file() true for it - for '<tempdir>\\NUL' and for bare 'NUL', plus the behavioural write/read-back half and the OS build string, as a paste-ready RECORD block. It has teeth in both directions: a LIVENESS CONTROL (an ordinary file must measure FILE_TYPE_DISK and is_file() true, so the probe cannot pass by calling everything a device); a hard failure if NUL is NOT a device on the build under test, because the #238 guard would then be refusing an addressable file - the data-destroying direction; and, when is_file() is true for the device, an assertion that validate_user_path refuses it with WindowsNullDevice, since that is the case where the NonRegularFile arm is structurally blind and the NAME guard is all that is left. Verified to COMPILE for Windows: cargo check -p wcore-tools --all-targets --target x86_64-pc-windows-gnu is clean on hetzner. It cannot be RUN from a Linux host - #[cfg(windows)], and a green from a host that cannot exhibit the property is worth nothing. THE RUN THAT SETTLES IT: on SeanDesktop (Win 11 26200) and, for the open residual, on a build-20348 host, 'cargo nextest run -p wcore-tools --no-capture -E \"binary(windows_nul_device_probe)\"'; paste the RECORD block into this note. WHAT CI ALREADY BUYS, so the ask is not overstated: the Windows leg of `CI (Array)` runs `just test-ci`, which is a workspace nextest, so this probe EXECUTES there on every run - a green Windows leg is already the assertion arm (NUL is still a device, and the name guard refuses it). What CI does NOT give is the RECORD: nextest captures stdout for a PASSING test, so the measured table only surfaces on failure or under --no-capture. So what is owed is the numbers, not the verdict. Blocked on maintainer because the Windows box is Sean-only infra."
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
    note: "Added 2026-08-29; this is the issue's second named mechanism and arguably the more serious half - a guard that vanishes exactly when the OS refuses to describe the target. Fixed at path_validation.rs:228: the if-let-Ok is now a match where NotFound alone still passes and every other ErrorKind returns a new Unstattable variant. The test asserts the PREMISE (a non-NotFound stat failure) before asserting the refusal, and a_not_yet_created_write_target_is_still_allowed is the control."
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
success. Everything here is not-met, and the scope question is deliberately
parked with the maintainer rather than resolved by a lane. Criteria come from
the cluster A verification note of 2026-08-29.
