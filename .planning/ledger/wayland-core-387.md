---
issue: 387
repo: FerroxLabs/wayland-core
kind: defect
title: "wl#1164's bash resolution breaks three Windows tests on any host with Git for Windows"
status: open
last_verified_commit: b52fb934
criteria:
  - id: c1
    text: "The three fixtures are written in the dialect the product actually resolved, read from the product rather than assumed from the platform"
    state: met
    evidence: "symbol:crates/wcore-config/src/shell.rs::shell_prefix_is_posix"
    owner: core
    note: "The answer comes from `bash_shell_argv_prefix()`, the SAME function BashTool calls to build the argv it runs, so the fixture and the subject can never disagree about which interpreter was picked. `shell_program_stem` / `shell_prefix_is_posix` live in wcore-config, the crate that BUILDS the prefix, and `wcore_tools::bash::shell_disclosure` was switched onto them - it carried a byte-identical copy of the same final-component parse, and two copies of this answer is exactly how the two could drift. The parse splits on / and \\ itself rather than via std::path, which on Unix does not treat \\ as a separator, so the Windows spellings classify identically from Linux and the helper is graded there: 71/71 in wcore-config on hetzner, including three new tests covering both separators, .EXE casing, an empty prefix, and agreement with every arm `bash_shell_prefix_for` can produce."
  - id: c2
    text: "Neither fixture is pinned with WAYLAND_BASH_SHELL=cmd, which would test an interpreter the product no longer uses on that host"
    state: met
    evidence: "symbol:crates/wcore-cli/tests/tool_formatter_real_payloads.rs::posix_interpreter"
    owner: core
    note: "No test sets WAYLAND_BASH_SHELL, and the expectations are still HARD-CODED per dialect rather than derived from the output under test - `echo_stdout_bytes()` returns a literal 15 or 16 and `stderr_then_fail()` returns one of two literal strings, selected on an INPUT (which interpreter was resolved). That distinction is load-bearing in this file: its own header says an expectation computed from the result under test cannot fail, which is the disease it was written to cure, and this change does not reintroduce it."
  - id: c3
    text: "The three named tests pass on a Windows host that HAS Git for Windows, measured, at retries 0"
    state: met
    evidence: "test:crates/wcore-tools/tests/bash_sandbox_routing_test.rs::env_and_cwd_are_honored_through_sandbox"
    owner: core
    note: "MEASURED ON SEANDESKTOP 2026-08-29 (Windows 11 build 10.0.26200.9168), tree D:\\u-win at b52fb934, `--retries 0`. HOST PRECONDITION CONFIRMED FIRST, because the defect is conditional and a host without Git for Windows could not exhibit it: `C:\\Program Files\\Git\\bin\\bash.exe` is present AND ahead of `C:\\WINDOWS\\system32\\bash.exe`, which is also present. Result, verbatim: `Summary [   0.200s] 6 tests run: 6 passed, 4421 skipped`, covering all three named tests plus three controls - `bash_failure_never_reports_exit_zero` (which uses `exit 3`, valid in both dialects, and was already passing before this change, so a regression in the shared plumbing would show), and the two path_validation arms carried by core#374. CONFIRMED ON THE LEG THAT REPORTED THE DEFECT, which is the arm that closes this rather than a workstation run: run https://github.com/FerroxLabs/wayland-core/actions/runs/33266399460, `CI (windows-latest, hosted)` - the same job where these three were 3 of the 9 failures in run 33258852685. They now PASS BY NAME and by index, so they ran and were not skipped: `PASS [   0.379s] ( 7958/15988) wcore-cli::tool_formatter_real_payloads bash_success_renders_the_real_exit_code_and_byte_count`, `PASS [   0.384s] ( 7959/15988) wcore-cli::tool_formatter_real_payloads bash_stderr_is_surfaced`, `PASS [   0.301s] (15382/15988) wcore-tools::bash_sandbox_routing_test env_and_cwd_are_honored_through_sandbox`. The leg is still red at `Summary [1400.572s] 15988 tests run: 15979 passed (4 slow, 1 flaky, 1 leaky), 9 failed, 163 skipped`, and NONE of the nine is one of these three: one is `wcore-cli::sandbox_activeness sandbox_status_filesystem_claim_matches_a_real_escape_attempt` and eight are `wcore-exec-backend` container/conformance cases that need a Docker daemon this image does not have. SECOND WINDOWS LEG, self-hosted msvc `CI (Array)` in the same run: `Summary [ 223.370s] 15988 tests run: 15987 passed (1 slow, 1 leaky), 1 failed, 163 skipped`, the one failure being that same `sandbox_activeness` case."
  - id: c4
    text: "The selection is non-vacuous: telling the fixtures the wrong interpreter reddens them on that same host"
    state: met
    evidence: "symbol:crates/wcore-config/src/shell.rs::shell_program_stem"
    owner: core
    note: "RED ARM on the same host and tree, committed before mutating. MUTATION, verbatim, and it lands on a CODE line not a comment - the `-` line is the function body: `-    matches!(shell_program_stem(prefix).as_str(), \"sh\" | \"bash\")` / `+    let _ = shell_program_stem(prefix);` / `+    false`. That makes the fixtures believe they are driving cmd while the product actually drives Git bash - the exact belief the pre-fix code held. RED: `Summary [   0.199s] 3 tests run: 0 passed, 3 failed, 4424 skipped`, all three named tests, with the load-bearing message `wrong exit code: exit 2 - 0 bytes stdout - 70 bytes stderr` at tool_formatter_real_payloads.rs:235 - the same `exit 2` #387 reported, because `exit /b 1` handed to a real bash is `exit` with two arguments. RESTORE VERIFIED: `git diff --stat` empty afterwards, file mtime bumped so cargo could not serve the mutated binary, and the re-run gives `Summary [   0.184s] 3 tests run: 3 passed, 4424 skipped`."
---

wl#1164 made the Windows shell resolve to Git for Windows' bash.exe when the
host has one. Three fixtures were written when Windows meant cmd, and they now
fail on any Windows host that has Git for Windows -- which includes GitHub's
windows-latest, where they were 3 of the 9 failures in run 33258852685.

The defect is in the fixtures, not in #1164: the feature's own behaviour was
confirmed live on the same host in the same session (wl#1164 c5).

THE CLASS WAS SWEPT, NOT ASSUMED. Every file referencing BashTool was checked
for a cfg(windows) arm: 13 files carry one. Three of them embed cmd dialect in a
fixture the product's own resolution can invalidate, and all three are changed.
The rest are dialect-agnostic -- `echo hello_stream` in
bash_sandbox_routing_test.rs:558 and bash/tests.rs:80, and
`echo typed-bypass-bash-succeeded` in wcore-agent -- and were left alone.

ONE FILE IS CMD-DIALECT AND DELIBERATELY NOT CHANGED, stated rather than left to
be found: `crates/wcore-tools/tests/win_toolchain_launch.rs` is full of cmd
spellings (`where git`, `echo hello> marker`,
`echo fn main(){println!("rc");}> m.rs && rustc ...`). It is outside the class
for a reason in the code, not because it is quiet: it is `#![cfg(windows)]`,
`#[ignore]`, and asserts on `WAYLAND_SANDBOX_LIVE_WINDOWS=1`, so it runs neither
in CI nor in the soak -- and when it IS run it drives the AppContainer backend,
where `downgrade_unsupported_shell_for_sandbox` (bash.rs) rewrites any
bash/sh/powershell prefix to `cmd /S /C` because the Low-integrity token runs
cmd.exe only. So under its own execution context the interpreter really is cmd
and its fixtures are correct. If that downgrade is ever removed, this file joins
the class.

NOTED, NOT FIXED: `downgrade_unsupported_shell_for_sandbox` carries a third copy
of the final-component `.exe`-stripping parse that `shell_program_stem` now
owns. It is not deduped here because, unlike `shell_disclosure`, it does not
have to agree with the fixtures -- it decides an argv rewrite on the sandbox
spawn path, and changing that path is risk this issue does not ask for.
