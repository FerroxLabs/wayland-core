---
issue: 388
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Long-running tasks intermittently truncate, stall, or restart inconsistently through Free Models Router"
status: open
last_verified_commit: be4467ed
criteria:
  - id: c1
    text: "Output caps are decided from what is actually known about the served model, not from the alias the request named"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c2
    text: "Reasoner replay is decided the same way, from what is known rather than from the alias"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c3
    text: "A prompt silently discarded by an under-sized served window is named to the user rather than showing as low pressure"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::ServedWindowTracker"
    owner: core
    note: "shipped under #1172; a different cause of the same reported symptom"
  - id: c4
    text: "Router failure is distinguishable from an upstream provider rate limit, and a failed response names the arm that was actually serving it"
    state: blocked
    owner: flux
    handoff: "FerroxLabs/wayland#1184"
    note: "RE-SCOPED AND SPLIT 2026-08-29. The old c4 read 'the remaining four bullets of this ticket's own Expected Behavior list are met' and handed the whole bag to flux. That was wrong, and the refutation is in a ticket the core lane itself filed: #1184 says in its own text that Flux declared the router-side truncate/stall causes fixed and live, that 'the remaining asks are harness-side, not Flux', and that #388's needs:flux label should be read as scoped to the failure-origin field and nothing else. The four bullets are now split out as c5 (met, core), c6 (not-met, core) and c7 (not-met, core). What genuinely cannot be decided from this repo stays here and is carried by #1184, which is open and needs:flux: a router rate limit and an upstream provider rate limit arrive as the same non-2xx from the same host, and x-flux-routed-model is absent on failed responses, so core would be guessing and presenting the guess as a classification"
  - id: c5
    text: "Expected-Behavior bullets 2 and 5: a length-cut response stops before any write and commits no speculative file change"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_388_output_truncation_test.rs::complete_tool_calls_in_a_length_cut_response_are_not_executed"
    owner: core
    note: "Shipped and, until this audit, UNCREDITED -- it was inside the old flux-owned c4, so two met bullets were being counted as somebody else's outstanding work. The #388(b) gate at engine.rs:15231 arms on the CUT rather than only on a call severed mid-argument, so a finish_reason=length response whose calls happened to close before the cut is still treated as the prefix of a plan whose remainder was discarded: the calls are dropped and the turn is retried once with a smaller-steps hint. Two controls in the same file stop it passing by refusing everything -- a_length_cut_text_answer_without_tool_calls_still_commits and the_same_tool_call_runs_when_the_response_is_not_truncated"
  - id: c6
    text: "Expected-Behavior bullet 3: a truncated or stalled long task preserves a checkpoint the user can continue from"
    state: not-met
    owner: core
    note: "MISLABELLED, corrected here. This sat inside the old flux-owned c4 and is core-engine work end to end. #1184 asserts that checkpoint/continue-from-checkpoint is 'being tracked on the core side'; a search of BOTH trackers on 2026-08-29 found no such issue, so nothing was tracking it. Session resume exists but is not this object -- the bullet asks for a checkpoint taken at the point a long task truncates or stalls, which the user can resume from, and the reporter's runs are exactly the case where a conversation is worth resuming and is not. Needs a core lane; it is not release-shaped work and should not be squeezed into 0.13.12 unscoped"
  - id: c7
    text: "Expected-Behavior bullet 7: the failure categories core CAN decide are machine-readable rather than a free-form string"
    state: not-met
    owner: core
    note: "MISLABELLED, corrected here -- also inside the old flux-owned c4. crates/wcore-protocol/src/events.rs:2290 is ErrorInfo { code: String, message: String, retryable: bool }: no typed category, no provider identity, no upstream status. Three of the five categories the ticket names -- context/token limit, tool/runtime failure, local Wayland error -- are decidable INSIDE core today and still reach the host as prose, so only the router-versus-provider split actually needs #1184. Note the overlap with wayland-core#314 c5, which is the same shape (an untyped Info frame where a typed one is owed) on the grant-refusal path; whoever takes either should look at both, since both are contract additions to the same event surface"
---

Graded against this ticket's own Expected Behavior list: 3 of 7 bullets are
met at v0.13.10, which is why it stays open.

Core's half was that output caps and reasoner replay were being decided from
`request.model` — the alias the caller typed — rather than from the model the
router actually served. `0cab1cf8` decides both from what is known.

The rest is the Free Models Router's side of the same symptom and is owned by
the flux lane, not core. #1172 closed a third, independent cause of the same
user-visible complaint (an endpoint silently discarding the prompt), which is
worth reading alongside this before anyone re-grades it.
