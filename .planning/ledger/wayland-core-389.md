---
issue: 389
repo: FerroxLabs/wayland-core
kind: defect
title: "Windows: a DETACHED_PROCESS quarantine child takes the user's console back with AttachConsole (split from #338 c2)"
status: open
last_verified_commit: a278f8c3
criteria:
  - id: c1
    text: "On Windows, a quarantine child that calls AttachConsole(ATTACH_PARENT_PROCESS) or AttachConsole(<wayland pid>) cannot end up sharing the user's console, measured by GetConsoleProcessList from inside that child"
    state: not-met
    owner: core
    note: "MEASURED FALSE, 2026-08-30, Windows 10.0.26200.9168, by the test this ticket asked for: `[plain] SHARES_USER_CONSOLE_BEFORE=true CONOUT_BEFORE=OPEN`; `[hardened] SHARES_USER_CONSOLE_BEFORE=false CONOUT_BEFORE=DENIED(6)`; `[hardened] ATTACH_PARENT_PROCESS=SUCCEEDED SHARES_USER_CONSOLE_AFTER=true CONOUT_AFTER=OPEN`; `[hardened] ATTACH_BY_EXPLICIT_PID=SUCCEEDED CONOUT_AFTER_EXPLICIT=OPEN SHARES_USER_CONSOLE_AFTER_EXPLICIT=true`; `[hardened] ALLOC_CONSOLE=SUCCEEDED SHARES_USER_CONSOLE_AFTER_ALLOC=false`; `[production_git] SHARES_USER_CONSOLE_BEFORE=false ... SHARES_USER_CONSOLE_AFTER_EXPLICIT=true`. Both spellings reach the operator's console, so reparenting the child is not a remedy either. Not reachable with process-creation flags: `CREATE_NO_WINDOW` (child already has a console, so AttachConsole would fail with ERROR_ACCESS_DENIED) is defeated by `FreeConsole` first, which is the sequence the probe runs. A real fix needs a session, window-station or AppContainer boundary. NOTE the AppContainer bar: that question is CLOSED and must not be reopened."
  - id: c2
    text: "OR, if c1 is judged unreachable with the available Win32 primitives, the other branch of #338's option menu is taken and graded -- a quarantine-originated prompt is LABELLED so the operator can tell it from a Wayland prompt, or the quarantine install refuses to run interactively on Windows -- and the choice is recorded in .planning/DECISIONS.md alongside Q-338c4 with its product cost"
    state: not-met
    owner: maintainer
    note: "UNGRADED and deliberately left open: this is a product decision with a real cost (labelling every quarantine operation, or refusing interactive plugin installs on Windows outright), and DECISIONS.md Q-338c4 currently records only the unix policy. The evidence needed to take it is now in the tree -- see c1's measurements and c3 -- so the decision is no longer blocked on measurement. Owner is maintainer, not core."
  - id: c3
    text: "The doc comment on harden_against_credential_prompt no longer asserts the DETACHED_PROCESS/setsid analogy without qualification"
    state: met
    evidence: "symbol:crates/wcore-cli/src/plugin/quarantine.rs::harden_against_credential_prompt"
    owner: core
    note: "Done 2026-08-30. The sentence `On Windows the analogue is DETACHED_PROCESS, which denies the child the parent\'s console and so CONIN$/CONOUT$ with it` is removed and replaced by the measured table, by `So on Windows this is a REDUCTION ..., not the elimination unix gets`, by the three foreclosed remedies, and by an explicit `Do not restore the analogy sentence: an overstated security guarantee is worse than an understated one, because it stops the next person looking`. The `DETACHED_PROCESS` constant's own comment now says `is CREATED with no console` and points at the test. RED ARM: the two mutations under #380 c1 both redden the test that carries these measurements, so the claim is anchored to something that fails when it stops being true."
---

Duplicate-by-independent-discovery of #380: both describe the same measured
Windows defect. #380 asked for the MEASUREMENT and for the product to stop
overclaiming; that is delivered and #380's ledger now reads met. THIS ticket
carries the live remainder, because its acceptance criteria ask for the
PROPERTY (c1) or an explicit product decision (c2), and neither has happened.
FerroxLabs/wayland-core#338 c2 is superseded into this issue.

Do not close this on c3 alone. c3 is the honesty fix; c1/c2 are the harm.
