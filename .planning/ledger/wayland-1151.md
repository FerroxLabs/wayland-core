---
issue: 1151
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Clean install of 0.12.4 / core 0.13.6 — the Bash tool is cmd.exe, and the transcript still assembles out of order"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The Bash tool discloses which shell it really runs, and the disclosure reaches the model"
    state: met
    evidence: "symbol:crates/wcore-tools/src/bash.rs::shell_disclosure"
    owner: core
    note: "reaches the model via the tool description (bash.rs:549 -> registry.rs:501). This TELLS rather than FIXES, which is the stated minimum and no more"
  - id: c2
    text: "A real bash is used when one is present, resolved explicitly and never System32\\bash.exe"
    state: met
    evidence: "symbol:crates/wcore-config/src/shell/windows_bash.rs::resolve_windows_bash"
    owner: core
    note: "closed with #1164 c1/c2 on lane/f13-win-bash. `bash_shell_argv_prefix` now returns `[<resolved bash>, \"-c\"]` on a Windows host with a real bash at a known install location and `cmd /S /C` on one without; System32\\bash.exe (WSL launcher) and the WindowsApps alias are refused by name. The LIVE half is not this criterion -- it is #1164 c5, still not-met, and no Windows box has run this branch yet"
  - id: c3
    text: "The transcript stops assembling out of order"
    state: blocked
    owner: desktop
    note: "no commit in this release touches ordering or assembly, and the symbols involved do not exist under crates/ at all — this half was never core's"
---

One of three sub-asks met in v0.13.10.

The reporter's minimum ask was to be told which shell the Bash tool is
actually running, because on a clean Windows install it was `cmd.exe` while
the tool is called Bash. That disclosure now reaches the model through the
tool description.

c2 is the real fix. It is now authored and graded on lane/f13-win-bash: the
selection is a pure function over an injected candidate list, so the
`System32\bash.exe` and WindowsApps refusals are unit-tested from Linux, and
`clippy --target x86_64-pc-windows-gnu` covers the `cfg(windows)` code a Linux
clippy is blind to. What is still owed is the thing cross-compilation cannot
give: a live run on SeanDesktop, tracked as #1164 c5 and still open. c3 is
Desktop's.
