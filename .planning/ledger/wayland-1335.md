---
issue: 1335
repo: FerroxLabs/wayland
kind: defect
title: "[Core] Active JSON-stream Stop drops accrued run usage and emits a zero-usage terminal"
status: closed
last_verified_commit: 1780b3164c6973279fd49d4dda17e8605f51d047
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

Verified on Hetzner with zero retries; receipt proof-1788759590-13903.json in stabilization execution evidence. Strict CLI all-targets clippy passed on production-equivalent 8dc531201. Source integrated; tracker closed with explicit Sean authorization after verification; publication pending. No claim of complete billing for unobserved provider responses.

Squash provenance reconciliation: PR #455 head `28634521b854d58672819843865f6ab7dd16d72c` and integrated
commit `1780b3164c6973279fd49d4dda17e8605f51d047` have identical tree
`90e028ae6de0537c60c1d86450ceb3083ae4610b`. Original anchor `04b2868032999942829bbf79e54672fafd74132b`
is an ancestor of the PR head; squash integration removed that ancestry. This
repin preserves the recorded grading and does not establish new runtime or
release acceptance. Exact-head CI `34242741624` passed before that squash.
