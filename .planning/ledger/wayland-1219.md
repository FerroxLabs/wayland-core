---
issue: 1219
repo: FerroxLabs/wayland
kind: defect
title: "In --json-stream an egress consent prompt is never sent to the host: the turn stalls 300s, then fails claiming the user declined"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "An EgressVerdict::Ask on the json-stream path reaches the host as an approval request the host can answer"
    state: met
    evidence: "test:crates/wcore-cli/tests/issue_1219_json_stream_egress_consent.rs::an_egress_ask_on_the_json_stream_path_reaches_the_host"
    owner: core
    note: "MET at 58eb9eac. `wcore_cli::json_stream_sink::build_json_stream_sink` is now the ONE construction of the --json-stream sink and calls `.with_hitl_suspend(true)`; main.rs calls it. The evidence test drives a real data-less GET to react.dev through the real `AgentEgressPolicy` + `BridgeConsentDoorbell` over that production builder, and asserts the host receives an `approval_required` frame carrying a call_id prefixed `egress:`, an `apr-` resume_token, and a context of kind egress_consent -- then a task that learned the token ONLY from that frame (never `bridge.pending_tokens()`, the shortcut that made a sibling test vacuous under wl#1180) resolves it, and the check returns Allow. RED ARM M1: delete `.with_hitl_suspend(true)` from build_json_stream_sink and this fails with 'the production json-stream sink was judged to have no approval surface' (nextest exit 100); it also reds 3 sibling tests, while the c2 and c3 tests stay green -- discriminating."
  - id: c2
    text: "No path installs BridgeConsentDoorbell where the sink cannot emit -- either with_hitl_suspend is called on the json-stream sink, or the doorbell is not installed there"
    state: met
    evidence: "test:crates/wcore-cli/tests/issue_1219_json_stream_egress_consent.rs::a_blocking_doorbell_is_never_installed_over_a_sink_that_cannot_prompt"
    owner: core
    note: "MET at 58eb9eac, by the FIRST disjunct AND a structural guard. bootstrap.rs no longer calls `policy.set_doorbell` directly; it goes through `wcore_agent::egress::install_consent_doorbell`, which returns false without installing when `output.approval_surface_available()` is false. That predicate is a new `OutputSink` trait method (default false) which `ProtocolSink` answers from `hitl_suspend_enabled` -- the SAME field `emit_approval_required` gates on, so the guard's answer cannot drift from what the emit actually does. TUI `ChannelSink` returns true (it sends unconditionally); ACP `RelaySink` delegates to the bound sink. The evidence test builds the exact pre-fix sink (every main.rs builder call EXCEPT with_hitl_suspend) and asserts the guard refuses it and `policy.has_doorbell()` stays false. RED ARM M2: delete the guard's early return and only this test plus its wcore-agent unit twin fail; all other 1219 tests stay green."
  - id: c3
    text: "A consent that was never shown is never reported to the user as 'declined at the consent prompt'"
    state: met
    evidence: "test:crates/wcore-cli/tests/issue_1219_json_stream_egress_consent.rs::a_consent_never_shown_is_not_reported_as_declined"
    owner: core
    note: "MET at 58eb9eac. `ConsentDecision` gained `Unavailable`: `BridgeConsentDoorbell::ask` now checks `approval_surface_available()` BEFORE registering anything on the bridge, so a prompt that cannot be shown returns immediately (no 300s park, no pending entry) and `resolve_ask` denies with a message that says it was 'refused without asking you' -- the string 'was declined at the consent prompt' is not in it. The evidence test installs the doorbell over a mute sink DIRECTLY, bypassing the c2 guard (c3 must hold even if some future path re-wires that), bounds the wait at 10s, and asserts BOTH that the deny text lacks the false decline AND that nothing was ever written to the wire -- so the first assertion cannot pass for the wrong reason. A control test (`an_actual_decline_is_still_reported_as_a_decline`) pins that a REAL operator deny still says so, so deleting the phrase would not pass. RED ARM M3: return `ConsentDecision::No` instead of `Unavailable` and this fails, printing the ticket's own text back: 'the user was blamed for declining a prompt that was never shown: Egress to `react.dev` was declined at the consent prompt.'"
  - id: c4
    text: "A test drives the json-stream sink through an egress Ask and asserts an ApprovalRequired frame is written; shown RED against today's hitl_suspend_enabled gate"
    state: met
    evidence: "test:crates/wcore-cli/tests/issue_1219_json_stream_egress_consent.rs::an_egress_ask_on_the_json_stream_path_reaches_the_host"
    owner: core
    note: "MET at 58eb9eac. Same test as c1 -- c1 is the behaviour, c4 is the requirement that a test pin it, and the criterion names one test doing both. It drives the PRODUCTION json-stream sink builder (not a lookalike assembled in the test, which would switch the flag on for the product and prove nothing) through a real EgressVerdict::Ask and asserts an ApprovalRequired frame is written to a captured wire. Shown RED against today's gate: RED ARM M1 removes `.with_hitl_suspend(true)` -- the only change -- and nextest exits 100 with 4 of 6 tests failing; restored with `git checkout --` plus a `touch` so cargo could not serve the mutated binary. Three further mutations (M2 install guard, M3 Unavailable->No, M4 the Unanswered message) each red a DIFFERENT test, so no test here rides on another's coverage."
---

In `--json-stream` mode an egress consent prompt is never sent to the host, and the turn silently stalls for 300s before failing closed with a message blaming a prompt the host was never shown. `ProtocolSink::emit_approval_required` returns early unless `hitl_suspend_enabled` is true — and `with_hitl_suspend(...)` is never called ANYWHERE in the workspace (`grep -rn with_hitl_suspend crates/ --include=*.rs` returns 4 hits: the setter definition and three doc comments; zero invocations, production or test). The json-stream sink at main.rs:5202-5215 is built without it, so the flag is permanently false. Yet bootstrap.rs:3014 installs `BridgeConsentDoorbell` on the session egress policy unconditionally whenever a session egress policy exists. So an `EgressVerdict::Ask` (policy.rs:154) -> `resolve_ask` -> `doorbell.ask()` -> `sink.emit_approval_required(...)` emits NOTHING, and `rx.await` (bridge_doorbell.rs:94) blocks until the reaper cancels it at `DEFAULT_APPROVAL_TTL` = 300s (approval.rs:40), yielding `ConsentDecision::No` -> `EgressDecision::Deny { reason: 'Egress to `host` was declined at the consent prompt...' }`. The doorbell's own comment at bridge_doorbell.rs:88-91 asserts 'this doorbell is only installed where a real surface exists' — on the json-stream path that premise is false. `emit_suspend` (protocol_sink.rs:1100) and `emit_approval_resume` (protocol_sink.rs:1216) are dead behind the same gate. The TUI is unaffected (tui/engine_bridge.rs:573 sends unconditionally), and ForgeFlow/Crucible are unaffected because engine.rs:17144 and engine.rs:26842 emit `ApprovalRequired` straight on the writer, bypassing the sink gate. `GatingProtocolWriter` (main.rs:4256) does not cover this either — it only synthesizes on `ToolRequest`, and an egress consent has no ToolRequest (its call_id is `egress:<uuid>`).

**Where.** crates/wcore-agent/src/output/protocol_sink.rs:1085 (the gate) + crates/wcore-cli/src/main.rs:5202 (sink built without with_hitl_suspend) + crates/wcore-agent/src/bootstrap.rs:3014 (doorbell installed unconditionally) + crates/wcore-agent/src/egress/bridge_doorbell.rs:88-94

**Why it matters.** User-visible on the Desktop host: a five-minute dead stall with no modal, then a false 'declined at the consent prompt' error. It also makes the seam #1180 just graded unreachable in production via the egress path on json-stream — the host can never be handed a resume_token to echo back, so `handle_approval_resume` can never resolve an egress consent there. That is the same shape as the bug #1180 was filed for, one layer up. No issue exists: `gh search issues --repo FerroxLabs/wayland hitl_suspend` returns nothing, and the 'egress consent' search returns only #1180/#583/#569/#497/#568, none of which is this.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.


## What changed beyond the four criteria (2026-08-30, lane dur-stalls)

One thing was fixed that no criterion names, and it is recorded here rather
than deferred. After c1/c2 the prompt IS shown on `--json-stream`; a user who
then simply does not answer within the 300s approval TTL still reached the
`ConsentDecision::No` arm and was told they had "declined at the consent
prompt". The prompt was shown, so c3 as written does not cover it -- but it is
the same lie, in the same `match`, and #1083 had already established for
`ApprovalCancelCause` that a bridge self-resolution must stay distinguishable
from a decision a human made. `ConsentDecision::Unanswered` now carries it,
mapped from `ApprovalOutcome.cancellation.is_some()` (TTL reap or host-stream
EOF) and worded as "no answer ... came back before it timed out". Pinned by
`an_unanswered_consent_is_not_reported_as_declined_either`, reddened by RED ARM
M4 and by nothing else.

Two consequences worth naming for a reviewer:

* `capabilities.hitl_suspend` is now advertised `true` in the `ready` frame on
  `--json-stream`, and `emit_suspend` / `emit_approval_resume` stop being dead
  code there. That is the intended blast radius of the first disjunct in c2;
  hosts that do not recognise the frames drop them per the W0 forward-additive
  decoder contract.
* On a sink with NO approval surface the doorbell is simply not installed, so
  an `Ask` verdict falls back to the documented no-doorbell posture (allow a
  data-less GET). The `Exfil` verdict stays hard-denied regardless, so this
  never widens the exfil boundary.
