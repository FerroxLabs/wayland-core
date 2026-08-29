---
issue: 380
repo: FerroxLabs/wayland-core
kind: defect
title: "On Windows the #338 credential-prompt elimination is bypassable: DETACHED_PROCESS is not setsid"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A Windows test drives harden_against_credential_prompt and establishes what a quarantine child can do to the user's console, exercising both AllocConsole() and AttachConsole(ATTACH_PARENT_PROCESS)"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D6, found while verifying FerroxLabs/wayland-core#338). Nothing has been done. The measured finding, verbatim: On Windows the #338 fix is bypassable, not merely untested. DETACHED_PROCESS (quarantine.rs:358-366) withholds the parent's console at creation time only; a console-less child may call AllocConsole() to make one, or AttachConsole(ATTACH_PARENT_PROCESS) to attach to its parent's console -- both documented Win32 behaviour. That is not the unix guarantee: a setsid'd child cannot reacquire the parent's ctty, because TIOCSCTTY refuses a terminal that is already another session's controlling terminal. So a third-party credential helper invoked by a quarantine clone on Windows can still raise an unattributable prompt on the user's console, which is the exact harm #338 describes."
  - id: c2
    text: "Either the Windows arm delivers the same elimination property as the unix arm -- the child cannot put a prompt on the user's console -- or the product states plainly that it does not and #338 c2 is scoped to unix in its own text"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D6). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "The Windows result is quoted VERBATIM from a real run on SeanDesktop; a cross-compile is not a runtime proof"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D6). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

On Windows the #338 fix is bypassable, not merely untested. DETACHED_PROCESS (quarantine.rs:358-366) withholds the parent's console at creation time only; a console-less child may call AllocConsole() to make one, or AttachConsole(ATTACH_PARENT_PROCESS) to attach to its parent's console -- both documented Win32 behaviour. That is not the unix guarantee: a setsid'd child cannot reacquire the parent's ctty, because TIOCSCTTY refuses a terminal that is already another session's controlling terminal. So a third-party credential helper invoked by a quarantine clone on Windows can still raise an unattributable prompt on the user's console, which is the exact harm #338 describes.

**Where.** crates/wcore-cli/src/plugin/quarantine.rs:358-366 (#[cfg(windows)] DETACHED_PROCESS); no Windows test exists -- crates/wcore-cli/tests/quarantine_terminal_authority.rs is `#![cfg(unix)]` at line 30 and is the only caller of harden_against_credential_prompt outside src/

**Why it matters.** The ledger's c4 note flags the Windows arm as 'unexercised', which reads as a coverage gap. It is a correctness gap: the chosen Windows primitive does not deliver the elimination property c2 is graded on. Closing #338 on unix evidence alone records the class as shut on a platform where it is still open, and there is no test that would ever catch it.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
