---
issue: 1335
repo: FerroxLabs/wayland
kind: defect
title: "[Core] Active JSON-stream Stop drops accrued run usage and emits a zero-usage terminal"
status: open
last_verified_commit: 3d2089b45
criteria:
  - id: c1
    text: "A real engine fixture that completes provider/tool work then receives host Stop reports accrued input/output/cache-read/cache-write delta exactly once with the same message correlation."
    state: not-met
    owner: core
    note: "Reported on pinned Core 6e4eca07fe5a215e365daa4e540767e0c9b8158b. Current-candidate reproduction and correction remain pending; active Stop accounting is distinct from normal completion and failed Spawn accounting."
  - id: c2
    text: "The next turn remains usable and normal completion is unchanged."
    state: not-met
    owner: core
    note: "Required cancellation/session-reuse regression coverage has not been executed for this issue."
  - id: c3
    text: "Stopping before any usage does not fabricate cost; do not invent usage for an in-flight provider response that never supplied it, and distinguish accrued partial usage from complete billing."
    state: not-met
    owner: core
    note: "Required no-usage and partial-usage acceptance remains pending. Desktop per-run delta mapping and runaway protection must be preserved."
---

Recorded from live #1335 on 2026-09-06. Tracker is OPEN, area:core, needs:core, with no milestone. Source lines in its report refer to the pinned release; they are not current-candidate proof. This ledger task neither fixes the defect nor authorizes wider implementation.
