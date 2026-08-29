---
issue: 1151
repo: FerroxLabs/wayland
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
    state: not-met
    owner: core
    note: "filed as #1164 and deliberately NOT shipped in 0.13.10: it needs live verification on real Windows hardware, and a cross-compile check is not verification. The silent `echo A; echo B` exit-0 still exists"
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

c2 is the real fix and is held back on purpose — it is a Windows behaviour
change and this repo has shipped Windows changes verified only by
cross-compilation before. c3 is Desktop's.
