---
issue: 1285
repo: FerroxLabs/wayland
kind: defect
title: "Two more macOS-only retry flakes: harness_tui_flow resume_repaints and wcore-mcp f016_real_spawn"
status: open
last_verified_commit: 67fa14db6
criteria:
  - id: c1
    text: "Both tests are either made deterministic under parallel nextest on macOS, or their allowlist entries are deleted at expiry because a normal-duration run stopped reproducing them."
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "Filed 2026-09-01. NOT FIXED IN 0.13.12 -- allowlisted with measured evidence and a 2026-10-01 expiry that says DELETE rather than renew. Milestoned 0.13.13. Both were observed on macos-latest only; Linux graded 0 unlisted for the same tests across the runs sampled."
---

# Two macOS retry flakes

Ledgered for coverage. Repair or expiry is 0.13.13 work.
