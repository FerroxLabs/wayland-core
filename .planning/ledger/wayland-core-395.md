---
issue: 395
repo: FerroxLabs/wayland-core
kind: defect
title: "engine.run() cost is ~linear in tool-result size (~100 s/MB in the test profile), and it is not the spill path"
status: open
last_verified_commit: 4a738f2e
criteria:
  - id: c1
    text: "The debug-vs-release question is SETTLED by measurement: the same probe is run under `--release` at 240,000 and 480,000 chars, and the per-byte term is either reproduced (a product finding) or shown to collapse (a test-profile artifact). Whichever it is, the numbers and the host load are recorded."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c2
    text: "If the per-byte term survives release, the function carrying it is NAMED by measurement — a profiler, or bisecting instrumentation inside `run_turn` — and not inferred. Two inferences are already recorded as unpromising above; naming a third by reading is not a close."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c3
    text: "A regression guard exists for whichever answer c1 gives: if it is a product cost, a test that fails when the per-byte term grows; if it is an artifact, the finding is recorded where the next person measuring a slow wcore-agent test will find it, so this is not re-derived a third time."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
