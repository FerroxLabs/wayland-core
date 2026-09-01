---
issue: 416
repo: FerroxLabs/wayland-core
kind: defect
title: "[nightly-windows-soak] FAIL - 2026-09-01"
status: open
last_verified_commit: 67fa14db6
criteria:
  - id: c1
    text: "The nightly Windows soak failure is triaged: either the failing soak assertion is fixed, or the run is shown to have failed for an infrastructure reason and the issue is closed by the maintainer."
    state: not-met
    evidence: "file:.github/workflows/nightly-windows-soak.yml"
    owner: core
    note: "Ledgered 2026-09-01 by the core lane during the 0.13.12 release gate. NOT TRIAGED, and this ledger does not pretend otherwise -- it exists because the coverage gate correctly refuses a release while an OPEN in-scope issue on either tracker has no ledger file at all, and this one was auto-filed by github-actions after this release's work had been graded. Milestoned 0.13.13. It carries the test-debt and windows-soak labels and was raised by the nightly soak, not by the 0.13.12 diff."
---

# An auto-filed nightly soak failure

Ledgered so the release gate can see it. Triage is 0.13.13 work.
