---
issue: 238
repo: FerroxLabs/wayland-core
title: "Windows reserved DOS device names (NUL/CON/COMn) bypass path guard"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A Write or Edit to a path whose final component is a Windows device does not report success while discarding the bytes"
    state: not-met
    owner: core
    note: "validate_user_path checks null bytes, UNC, device/verbatim prefixes, absoluteness, traversal, system paths and non-regular files, and nothing else. Silent data loss with a false success claim is the only part of this issue worth spending on"
  - id: c2
    text: "The guard refuses only names that measurement shows are still devices on the build under test"
    state: not-met
    owner: core
    note: "on Win 11 build 26200 only bare NUL is a device; CON, AUX, PRN, COM1, LPT1, NUL.txt and AUX.log all wrote and read back as ordinary files. The fix filed in the issue body would refuse real user data"
  - id: c3
    text: "Wrong-refusal controls pin that aux.txt, NUL.txt, COM1, con.json and nul.rs still validate Ok"
    state: not-met
    owner: core
    note: "these controls are the test. A reserved-name guard was already written once, unit-tested green and discarded, because the tests encoded the same wrong assumption the issue does"
  - id: c4
    text: "A Windows probe records whether bare NUL is a device on the build under test and whether fs::metadata reports is_file() true for it"
    state: not-met
    owner: core
    note: "the NonRegularFile check at path_validation.rs:152 is reasoned to be structurally blind on Windows because FileType::is_file() has no character-device concept there. That is reasoning, not measurement, and 20348 has never been probed at all"
  - id: c5
    text: "The scope is decided - narrow to bare NUL, or close as wont-fix with the 26200 measurement recorded"
    state: blocked
    owner: maintainer
    note: "the issue asks for the textbook reserved-name list, which measurement refutes. Building the wrong guard costs real user files, so the scope is a maintainer call and must not be picked silently by a lane"
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
