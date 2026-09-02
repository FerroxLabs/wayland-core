---
issue: 1284
repo: FerroxLabs/wayland
kind: defect
title: "Flaky on macOS only: the_live_backend_timeout_bounds compares two wall-clock samples taken under different load"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "The test stops deciding on a ratio between two wall-clock samples taken at different moments under different load, or its allowlist entry is deleted because the ratio was measured stable."
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "Filed 2026-09-01 during the 0.13.12 CI gate. NOT FIXED IN 0.13.12 -- the entry is an allowlist line with a measured rate and a 2026-10-01 expiry, not a repair. Milestoned 0.13.13 so it is tracked and is explicitly not part of this release's definition of done. The allowlist entry carries the measurement and the negative control; this ledger exists so the criterion has a named owner rather than living only in a dated comment."
---

# A ratio test on a shared runner

Recorded so the 0.13.12 coverage gate has a ledger for it. The repair is 0.13.13 work.
