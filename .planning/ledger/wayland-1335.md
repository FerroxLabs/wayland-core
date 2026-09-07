---
issue: 1335
repo: FerroxLabs/wayland
kind: defect
title: "[Core] Active JSON-stream Stop drops accrued run usage and emits a zero-usage terminal"
status: open
last_verified_commit: 04b2868032999942829bbf79e54672fafd74132b
criteria:
  - id: c1
    text: "A real engine fixture that completes provider/tool work then receives host Stop reports accrued input/output/cache-read/cache-write delta exactly once with the same message correlation."
    state: met
    evidence: "test:crates/wcore-cli/tests/smoke_p0.rs::stop_mid_turn_does_not_strand_json_stream_session"
    owner: core
    note: "Real binary smoke stop_mid_turn_does_not_strand_json_stream_session passes: accrued input12/output25/cache-write7/cache-read9 retained in exactly one m1 terminal and matching delta."
  - id: c2
    text: "The next turn remains usable and normal completion is unchanged."
    state: met
    evidence: "test:crates/wcore-cli/tests/smoke_p0.rs::stop_mid_turn_does_not_strand_json_stream_session"
    owner: core
    note: "Same fixture m2 completes normally with cumulative22/45 and delta10/20; cancelled engine future finishes durable cleanup before terminal emission."
  - id: c3
    text: "Stopping before any usage does not fabricate cost; do not invent usage for an in-flight provider response that never supplied it, and distinguish accrued partial usage from complete billing."
    state: met
    evidence: "test:crates/wcore-cli/tests/smoke_p0.rs::stop_mid_turn_does_not_strand_json_stream_session"
    owner: core
    note: "Same fixture m3 stops before delayed provider usage: input/output zero and no usage_delta; no inherited or invented provider usage."
---

Verified on Hetzner with zero retries; receipt proof-1788759590-13903.json in stabilization execution evidence. Strict CLI all-targets clippy passed on production-equivalent 8dc531201. Source integrated; tracker remains open pending release. No claim of complete billing for unobserved provider responses.
