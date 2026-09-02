---
issue: 1164
repo: FerroxLabs/wayland
kind: defect
title: "Windows: use a real bash when one is present, resolved explicitly (never System32 bash.exe)"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "Git Bash is resolved by known install location rather than by a bare PATH lookup for bash.exe"
    state: met
    evidence: "test:crates/wcore-config/src/shell/windows_bash_tests.rs::candidates_are_known_install_locations_never_a_bare_path_lookup"
    owner: core
    note: "`windows_bash_candidates` enumerates %ProgramFiles% / %ProgramW6432% / %ProgramFiles(x86)%\\Git and %LOCALAPPDATA%\\Programs\\Git, each as bin\\bash.exe then usr\\bin\\bash.exe, deduped case-insensitively. There is NO PATH lookup anywhere on the path: the test asserts the generated list and that an empty environment yields no candidates at all rather than a bare name"
  - id: c2
    text: "System32 bash.exe and the WindowsApps shim are explicitly refused as bash candidates"
    state: met
    evidence: "symbol:crates/wcore-config/src/shell/windows_bash.rs::windows_bash_path_refusal"
    owner: core
    note: "refused by NAME, not by hoping they are absent -- the WSL launcher is a real executable that exits 0 while running against /mnt/c. Covers System32/Sysnative/SysWOW64 and any WindowsApps component, both slash spellings, case-insensitive; relative, bare and UNC/device paths are refused too (an agent can write a bash.exe into the workspace). RED ARM: neutering the System32 arm to `if false && [...]` reddened system32_bash_is_refused_as_the_wsl_launcher with `left: None / right: Some(WslLauncher)` plus 2 more"
  - id: c3
    text: "The selection is a pure function over an injected candidate list, unit-testable from any host"
    state: met
    evidence: "symbol:crates/wcore-config/src/shell/windows_bash.rs::select_windows_bash"
    owner: core
    note: "takes `&[BashCandidate]` where presence is INJECTED, so every branch including both refusals is reached from Linux; `windows_components` splits on / and \\ itself rather than std::path, which on Unix does not treat \\ as a separator. Only `resolve_windows_bash` reads the environment and probes. 15 tests in windows_bash_tests.rs, all host-independent, green on hetzner Linux"
  - id: c4
    text: "With no acceptable bash found, execution falls back to cmd and the #1151 disclosure names cmd to the model"
    state: met
    evidence: "test:crates/wcore-tools/src/bash/tests.rs::no_acceptable_bash_falls_back_to_cmd_and_the_disclosure_says_so"
    owner: core
    note: "both halves in one test so the fallback cannot be split from its disclosure. `bash_shell_prefix_for(true, _, None)` returns the cmd /S /C prefix (windows_prefix_falls_back_to_cmd_without_an_acceptable_bash) and the disclosure built from THAT prefix still says `cmd.exe, NOT bash`. shell_disclosure also had to start reading the FINAL path component: given the absolute bash path it previously emitted \"The interpreter is `c:\\program files\\git\\bin\\bash`, NOT bash\" -- quoted verbatim from the red arm"
  - id: c5
    text: "A live confirmation run on SeanDesktop is recorded before the change lands"
    state: met
    evidence: "symbol:crates/wcore-config/src/shell/windows_bash.rs::resolve_windows_bash"
    owner: core
    note: "RUN ON SEANDESKTOP 2026-08-29 (Windows 11 build 26200), tree ab6b602f in D:\\wf13w, through the compiled binary and the agent's OWN shell tool - `wayland-core sandbox exec` dispatches to `wcore_tools::bash::BashTool::execute_with_ctx`, the same function the model drives, not a sibling path. Host precondition confirmed first, because the hazard is conditional: `C:\\Program Files\\Git\\bin\\bash.exe` present AND `C:\\Windows\\System32\\bash.exe` present, so both the right answer and the wrong one were reachable. (a) `sandbox exec \"echo A; echo B\"` -> `Exit code: 0`, STDOUT two lines, `A` then `B`. Before this change the same input printed the single literal line `A; echo B`. (a2) THE WSL CONTROL, which is the arm that matters: `sandbox exec \"pwd\"` -> `/d/wf13w`. That is the MSYS form Git for Windows produces; the System32 WSL launcher would have produced `/mnt/d/...` against a different filesystem. So the resolver reached Git bash and not the launcher, measured rather than assumed. (b) `WAYLAND_BASH_SHELL=cmd` -> `A; echo B`, one literal line: an operator who chose cmd still gets cmd, and this doubles as the pre-change control since it is byte-identical to what (a) used to print. (c) NO ACCEPTABLE BASH, produced by pointing %ProgramFiles% / %ProgramW6432% / %ProgramFiles(x86)% / %LOCALAPPDATA% at an empty directory so `windows_bash_candidates` yields nothing - the product's real code path, and safer than renaming a binary out of Program Files on the operator's machine: `echo A; echo B` -> `A; echo B`, i.e. the fallback is cmd; and `pwd` -> `/d/wf13w`, NOT a `/mnt/` path, so the fallback did not reach System32 bash.exe. (d) THE DISCLOSURE, read LIVE from this host rather than from an injected-candidate unit test: `BashTool::description()` ends `- Host OS: windows. Your command string is handed to `C:\\Program Files\\Git\\bin\\bash.exe -c`, and THAT interpreter, not bash, decides what it means.` / `- This is a real bash. POSIX shell syntax applies in full: `;` separates commands, `$VAR` expands, and pipes, redirection, globbing, `[[ ]]`, arrays and heredocs all work.` So the description names the RESOLVED PATH, which is what c5 asked for. NOT EXERCISED, and stated rather than implied: the AppContainer arm of (d) in the original ask - `WAYLAND_SANDBOX=appcontainer` with `downgrade_unsupported_shell_for_sandbox` - was not run here. That surface is the subject of FerroxLabs/wayland-core#368 and #369 on the same host and should be graded there, not folded into this note. WHAT THE LIVE RUN CAUGHT, WHICH IS THE REASON THIS CRITERION EXISTS: the change is CORRECT and it BROKE THREE WINDOWS TESTS, and it had already merged into integ/f13 before anyone ran it on Windows. `wcore-tools::bash_sandbox_routing_test::env_and_cwd_are_honored_through_sandbox`, `wcore-cli::tool_formatter_real_payloads::bash_success_renders_the_real_exit_code_and_byte_count` and `::bash_stderr_is_surfaced` fail on any Windows host that has Git for Windows - which includes GitHub `windows-latest`, where they are 3 of the 9 failures in run 33258852685. A/B on one host, one tree (362ba8a6), back to back, retries=0, interpreter the ONLY variable: ARM A default (Git bash resolved) `Summary [   3.152s] 3 tests run: 0 passed, 3 failed, 4424 skipped`; ARM B `WAYLAND_BASH_SHELL=cmd` `Summary [   0.185s] 3 tests run: 3 passed, 4424 skipped`. The 20x runtime gap between arms is a second signature that the arms really are different interpreters. The three fixtures embed cmd semantics - %VAR% expansion a POSIX shell does not do, a byte count that moves with CRLF vs LF, and an exit code cmd and bash disagree on. Filed as FerroxLabs/wayland-core#387, which explicitly forbids the tempting fix of pinning them with WAYLAND_BASH_SHELL=cmd, since that would test an interpreter the product no longer uses there. c5 is MET - the run was done and is recorded, findings included - but #1164 itself should not be considered landed until #387 is closed. FOLLOW-UP CLOSED 2026-08-29 on lane/f13-u-win-native: #387 is fixed and graded on the same box. The three fixtures now read the resolved interpreter from the product (`wcore_config::shell::shell_prefix_is_posix`) instead of assuming the platform names it, and on SeanDesktop, Windows 11 build 10.0.26200.9168, tree D:\\u-win at b52fb934. HOST PRECONDITION CONFIRMED FIRST, because the whole defect is conditional: `C:\\Program Files\\Git\\bin\\bash.exe` is present AND ahead of `C:\\WINDOWS\\system32\\bash.exe`, which is also present -- so both the right interpreter and the wrong one were reachable on the box that graded this. they run `Summary [   0.200s] 6 tests run: 6 passed, 4421 skipped` (the three plus three controls). RED ARM, same host: neutering the selector to `false` -- i.e. telling the fixtures they are driving cmd while the product drives Git bash -- gives `Summary [   0.199s] 3 tests run: 0 passed, 3 failed, 4424 skipped`, and restoring gives `Summary [   0.184s] 3 tests run: 3 passed, 4424 skipped`. So #1164 is now landable: the feature was already correct, and its blast radius is closed and measured rather than argued."
---

Follow-up to #1151, which closed the disclosure half: the model is now told
which interpreter it is really driving. This is the other half — actually using
bash on Windows when a real one is present.

The #1151 lane declined the work believing `bash.exe` on a default Windows PATH
is the WSL launcher. Measured on SeanDesktop, that premise is wrong there: Git
for Windows puts `bin\bash.exe` first and the WSL launcher is third. But the
hazard is real and conditional — on a box without Git for Windows, a naive PATH
lookup lands on the WSL launcher and runs against a different filesystem
entirely. Hence c1 and c2: resolve explicitly, refuse explicitly.

The issue also flags surface not yet measured: the AppContainer backend's
`classify_bare_shell` / `resolve_program` rejects bare `powershell`/`pwsh` and
would need an arm, and the cmd-payload quoting contract needs a third arm for
POSIX-quoted payloads.

Built on lane `lane/f13-win-bash`. The resolver is
`crates/wcore-config/src/shell/windows_bash.rs`, wired in through
`bash_shell_argv_prefix`, which now returns `[<bash>, "-c"]` on a Windows host
that has one and `["cmd", "/S", "/C"]` on one that does not. Precedence is
unchanged otherwise: an explicit `cmd`/`powershell`/`pwsh` still wins, and only
an unset setting or an explicit `bash`/`sh` reaches the resolver -- so an
operator who already chose does not even pay for the probe.

Two adjacent surfaces moved because this change would otherwise have broken
them, and neither is a new feature:

* `shell_disclosure` derives the interpreter from the FINAL path component. It
  read the whole string, which turned an absolute bash path into the
  unrecognized-shell arm and told the model bash is not bash.
* the AppContainer sandbox runs cmd.exe and nothing else -- its own
  `classify_bare_shell` says git-bash cannot load `msys-2.0.dll` under the
  Low-IL token. `downgrade_powershell_for_sandbox` is now
  `downgrade_unsupported_shell_for_sandbox` and covers bash/sh, so an
  AppContainer host keeps working instead of hard-failing every command.

The cmd-payload quoting contract is untouched and does not need a third arm
for this change: `cmd_payload_index` returns `None` for a bash argv, so
`quote_cmd_payload` and `reject_undeliverable_cmd_payload` (the CR/LF refusal)
apply only to cmd, and a bash payload goes through ordinary argv passing.
