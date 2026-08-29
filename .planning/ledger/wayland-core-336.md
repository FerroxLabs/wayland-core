---
issue: 336
repo: FerroxLabs/wayland-core
title: "Flaky: harness_tui_flow narrow_terminal_resize_stays_coherent_without_panicking times out under parallel load"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "PtyHarness::resize resizes the vt100 parser as well as the PTY master"
    state: not-met
    owner: core
    note: "the parser grid is created 40x120 and never told about a resize; there is no set_size call anywhere in the file. vt100::Parser::set_size exists in the pinned 0.15.2, so no version bump is needed"
  - id: c2
    text: "The post-resize predicate can only be satisfied by a frame that is actually 80 columns wide"
    state: not-met
    owner: core
    note: "today it waits for WAYLAND and Workspace, which boot_to_workspace already waited for, so the wait can be satisfied by pre-resize residue in the stale 120-wide grid. The test is vacuously passable in one direction"
  - id: c3
    text: "Making PtyHarness::resize a no-op turns the test red"
    state: not-met
    owner: core
    note: "on the shipped tree that mutation still PASSES, which is the proof the test is half-vacuous. A second mutation - a render panic below 100 columns - must fail both before and after"
  - id: c4
    text: "The flake rate is re-measured at retries=0 over N of at least 20 with a known-positive control in the same run"
    state: not-met
    owner: core
    note: "the issue's own first figure of four in eight was discarded because an empty grep is not a verdict. Raising the 5s budget would make the timeout rarer and leave the vacuity untouched - green bought on a test that already cannot fail in one direction"
---

The reported symptom is a PTY test that times out about one run in six under
parallel load, waiting up to five seconds after a terminal resize.

The root cause is not the budget. The harness resizes only the PTY master and
never tells the vt100 parser, so the parser keeps a stale 120-column grid for
the life of the test. Two separate defects sit on that one line: the wait
predicate is already true before the resize, so the test can pass without ever
observing one; and it can only time out when the app clears the grid and is slow
to repaint into a mis-sized parser, which is the flake.

Nothing masks this today. It is not in the flaky allowlist and it has no nextest
override, contrary to what the triage brief claimed, so it costs real CI reds
now. It is a single test file, cfg(unix), and fully verifiable on hetzner.

Criteria come from the cluster C verification note of 2026-08-29.
