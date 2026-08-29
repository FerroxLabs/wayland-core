---
issue: 336
repo: FerroxLabs/wayland-core
title: "Flaky: harness_tui_flow narrow_terminal_resize_stays_coherent_without_panicking times out under parallel load"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "PtyHarness::resize resizes the vt100 parser as well as the PTY master"
    state: met
    evidence: "symbol:crates/wcore-cli/tests/harness_tui_flow.rs::resize"
    owner: core
    note: "The body calls parser.set_size(rows, cols) before master.resize(). set_size rather than a fresh Parser, deliberately: a cleared grid breaks the diff-renderer and was measured to fail the chrome wait on a healthy binary."
  - id: c2
    text: "The post-resize predicate can only be satisfied by a frame that is actually 80 columns wide"
    state: met
    evidence: "symbol:crates/wcore-cli/tests/harness_tui_flow.rs::widest_painted_row"
    owner: core
    note: "DEVIATION FROM THE WORDING, flagged. The discriminating predicate is at 140 columns, not 80: the test shrinks to 80 and asks the PROCESS about survival, then widens to 140 (past the 120 boot grid) and requires wait_for_width(140), so a glyph past column 120 cannot be pre-resize residue. 536fbfbe removed the post-shrink screen predicate because a healthy binary need not fully repaint at the intermediate width. widest_painted_row reads the GRID cell by cell rather than screen_text(), because vt100 contents() re-joins wrapped rows and would have passed against the defect."
  - id: c3
    text: "Making PtyHarness::resize a no-op turns the test red"
    state: not-met
    owner: core
    note: "Structurally a no-op resize leaves the parser at 120 so wait_for_width(140) should time out, and the shrink arm's assert!(h.is_running()) covers the render-panic mutation. But this arm depends on the binary repainting, and that repaint has ITSELF been observed flaky in this very test (one full-suite run in six timed out at the intermediate width). MUTATION ARM NOT RUN. The structural argument is recorded above, but this criterion asserts an OBSERVED outcome and nothing in the tree records one. The standing rule in this repo is that a test nobody watched fail is not evidence, so it grades not-met until one cheap run flips it."
  - id: c4
    text: "The flake rate is re-measured at retries=0 over N of at least 20 with a known-positive control in the same run"
    state: not-met
    owner: core
    note: "Requires a MEASURED rate at retries=0 over N of at least 20 with a known-positive control in the same run. No such measurement exists anywhere in the tree; a code change cannot satisfy it. AGGRAVATING: this test is in NEITHER .config/flaky-allowlist.txt NOR any nextest retries=0 override, so it costs real CI reds today, and PAINT_BUDGET was raised 5s to 30s without re-measuring the rate."
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
