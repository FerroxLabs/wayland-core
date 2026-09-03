---
issue: 434
repo: FerroxLabs/wayland-core
kind: defect
title: "The #338 c2 Windows residual pin reports the escape CLOSED while the same report shows it open"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "The pin no longer depends on the PARENT owning the operator console: it is either re-anchored on the _EXPLICIT probe, which does not, or it asserts the precondition (CONSOLE_WINDOW_AT_CREATION != NONE) and skips honestly when it does not hold."
    state: not-met
    owner: core
    note: "Candidate mechanism, NAMED AND NOT PROVEN. ATTACH_PARENT_PROCESS attaches to whatever console the parent owns, and the failing report's own first line is CONSOLE_WINDOW_AT_CREATION=NONE. A headless CI parent with no console to donate can make SHARES_USER_CONSOLE_AFTER read false for a reason that has nothing to do with containment. The _EXPLICIT probe does not depend on the parent and did not move in the same report."
  - id: c2
    text: "A failure of this pin can no longer be read as 'the residual closed' while another field in the same report shows the child reaching the operator console: the assertion message names BOTH fields."
    state: not-met
    owner: core
    note: "This is the criterion that matters, and it is about the MESSAGE, not the probe. The comment above the pin instructs the reader to delete and invert the block if it stops holding. In run 33713740549 it stopped holding while SHARES_USER_CONSOLE_AFTER_EXPLICIT=true sat eight lines below in the same string. Following the instruction would have inverted a security pin against evidence inside its own payload."
  - id: c3
    text: "The test comes off .config/flaky-allowlist.txt, and the entry is DELETED rather than renewed."
    state: not-met
    owner: core
    note: "Listed 2026-09-03 with a 2026-09-20 expiry."
  - id: c4
    text: "NOT MEASURED, and recorded as such: the rate, and whether the _EXPLICIT arm ever moves."
    state: not-met
    owner: core
    note: "Observed ONCE, retried into a pass, across the 12 runs in which report failed continuously. Not reproduced on SeanDesktop -- that host has an interactive session, which is exactly the condition the hypothesis says HIDES it, so its green is not evidence against. The _EXPLICIT arm has not been observed to move in any stored artifact, which is the reason the escape is described as still open rather than as intermittent."
---

# A canary that fires in the safe-looking direction

The pin records a MEASUREMENT, not a correctness claim: that `DETACHED_PROCESS`
is weaker than `setsid`, and that #338 c2 is therefore satisfied by elimination
on unix ONLY. Its failure mode is the dangerous one, because the reading it
invites -- "the residual closed, re-grade c2 on Windows" -- is contradicted by
`SHARES_USER_CONSOLE_AFTER_EXPLICIT=true` in the very report it prints.

Nothing got safer. The child still reached the operator console; only the
probe that depends on the parent's console became unstable.
