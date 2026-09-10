---
issue: 1353
repo: FerroxLabs/wayland
kind: defect
title: "Concurrent effect-checkpoint stores can jointly exceed the session quota"
status: open
last_verified_commit: cd85dac88
criteria:
  - id: c1
    text: "Concurrent stores into one checkpoint directory cannot jointly exceed the session quota: the probe test goes GREEN with the repair and RED with the repair removed (a red arm from history); a store the quota refuses still fails closed and a store that fits still succeeds."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. REPRODUCED, not inferred: the probe `concurrent_checkpoint_stores_cannot_jointly_exceed_the_session_quota` at wayland-core 506aed92e (a separate probe branch of the w15/win1301 lane, NOT a fix, NOT to be merged as-is -- it adds a cfg(test) interleaving gate after the quota check in `store_effect_checkpoint`) fills the `.effects` directory to leave room for exactly one maximum-size checkpoint, lets two stores both pass the check before either writes, and ends at 570,425,344 bytes against the 536,870,912-byte MAX_EFFECT_CHECKPOINT_SESSION_BYTES quota, assert failing at session_journal.rs:4244. CAUSE AS READ from the code: no lock is held across the session-quota scan and the write it admits; `effect_checkpoint_path` takes the journal writer lock only to read the journal path. Two production writers share the directory: the durable child result store (durable_spawner, under its own `mutations` lock) and the tool-effect preimage store (orchestration/mod.rs:1215, via spawn_blocking, no shared lock). NOT CLAIMED: the gate FORCES the interleaving; how often production concurrency reaches the window is not measured."
  - id: c2
    text: "The repair does not serialize unrelated sessions: any quota reservation or lock is scoped to one checkpoint directory, shown by a test that two different sessions storing concurrently are not blocked by each other."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10."
  - id: c3
    text: "Crash-left temporary checkpoint files (`.{digest}.*.tmp`) are still counted against the quota after the repair, pinned by a test."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. Recorded because the wayland#1301 c3 lane is separately changing HOW this scan sizes entries (directory-listing sizes for published, immutable checkpoints only); the two changes touch the same function and must land in order."
  - id: c4
    text: "The wayland#1301 c3 per-dispatch timing is not regressed by the repair: the isolated fix1_dispatch_budget_aborts_with_partial_result step is measured before and after on the same executor class."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. A lock held across a directory scan is exactly the kind of change that can add per-dispatch cost, and #1301 c3 is graded on that cost. Trackers were searched open and closed for 'quota', 'effect checkpoint', 'effects directory' and 'disk exhaustion' in both repos with no matching ticket; control query 'relay overloaded' returned #1352 in the same session, so the search works."
---

Created 2026-09-10 at filing time, not retroactively. Found by the w15/win1301
lane while attributing the Windows per-dispatch I/O cost behind wayland#1301 c3.
