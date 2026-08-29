---
issue: 1164
repo: FerroxLabs/wayland
kind: defect
title: "Windows: use a real bash when one is present, resolved explicitly (never System32 bash.exe)"
status: open
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
    state: not-met
    owner: core
    note: "NEEDS-PLATFORM-RUN, and nothing on a Linux host can substitute. Everything else is authored and green. The run: on SeanDesktop at this branch, (a) with Git for Windows installed, `wayland-core` Bash `echo A; echo B` must print two lines (today it prints the literal `A; echo B` and exits 0) and the tool description must name the resolved bash path; (b) `WAYLAND_BASH_SHELL=cmd` must still be cmd; (c) rename the Git bin\\bash.exe away and confirm the fallback is cmd, NOT System32\\bash.exe reaching /mnt/c -- `pwd` must not print a /mnt/c path; (d) if WAYLAND_SANDBOX=appcontainer is exercised, confirm the downgrade keeps commands running"
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
