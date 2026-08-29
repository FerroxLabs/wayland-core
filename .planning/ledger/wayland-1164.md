---
issue: 1164
repo: FerroxLabs/wayland
title: "Windows: use a real bash when one is present, resolved explicitly (never System32 bash.exe)"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "Git Bash is resolved by known install location rather than by a bare PATH lookup for bash.exe"
    state: not-met
    owner: core
    note: "a naive PATH lookup is the design the issue explicitly refuses; PATH order was measured on one machine only"
  - id: c2
    text: "System32 bash.exe and the WindowsApps shim are explicitly refused as bash candidates"
    state: not-met
    owner: core
    note: "System32 bash.exe is the WSL launcher and would silently execute against /mnt/c paths; the shim may be a store stub"
  - id: c3
    text: "The selection is a pure function over an injected candidate list, unit-testable from any host"
    state: not-met
    owner: core
    note: "this is how #1151 graded its Windows wording, and this issue cannot be graded from Linux otherwise"
  - id: c4
    text: "With no acceptable bash found, execution falls back to cmd and the #1151 disclosure names cmd to the model"
    state: not-met
    owner: core
  - id: c5
    text: "A live confirmation run on SeanDesktop is recorded before the change lands"
    state: not-met
    owner: core
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
POSIX-quoted payloads. Nothing has been built; the only `Program Files\Git`
string in the tree is a #1151 disclosure test fixture, not a resolver.
