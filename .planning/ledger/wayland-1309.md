---
issue: 1309
repo: FerroxLabs/wayland
kind: defect
title: "raw_mode_with_nothing_typed_still_denies: the pty capture ends at the prompt, so a missing denial reason and a truncated read are indistinguishable"
status: open
last_verified_commit: 2347d8f9
criteria:
  - id: c1
    text: "The failure distinguishes the two readings: the assertion waits for a terminal condition rather than sampling whatever the pty holds at one moment, and on failure reports how long it waited."
    state: not-met
    owner: core
    note: "Filed 2026-09-03 from run 33752019921 (linux-containerized), approval_pty_raw_partial_line.rs:285:5 after 2.011s. READ THE PAYLOAD: the denial is not missing. The capture shows the prompt rendered and the tool call described, and what is absent is the REASON text the assertion looks for. It ends exactly at the '> ' prompt, which is what a read returning before the next write looks like."
  - id: c2
    text: "With the denial-reason write deliberately suppressed, the test still reds."
    state: not-met
    owner: core
    note: "The load-bearing criterion. The cheap fix for c1 -- wait longer, or relax the assertion -- can convert this into a test that cannot fail, and it grades a user-facing property: whether an operator is told WHY a tool call was denied. A red arm is the only thing that separates a fix from a silencing."
  - id: c3
    text: "Measured on Linux at --retries 0, n>=20, under full-workspace contention rather than alone."
    state: not-met
    owner: core
    note: "RATE NOT MEASURED -- observed once. Alone is the wrong instrument for a capture race: contention is what decides whether a read lands early."
  - id: c4
    text: "The entry comes off .config/flaky-allowlist.txt and is DELETED rather than renewed."
    state: not-met
    owner: core
    note: "Listed 2026-09-03 with a 2026-09-20 expiry. The entry deliberately does NOT pick between the two readings, and neither does this ledger."
---

# Filed from the fold-in run, not the original outage window

Bringing wayland-core#433 into wayland-core#432 produced a new tree, and a new
tree is a new sample. The retry-flake gate can only ever report whichever
member fires, so this cluster was invisible until then.
