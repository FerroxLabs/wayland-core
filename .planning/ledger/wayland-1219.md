---
issue: 1219
repo: FerroxLabs/wayland
kind: defect
title: "In --json-stream an egress consent prompt is never sent to the host: the turn stalls 300s, then fails claiming the user declined"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "An EgressVerdict::Ask on the json-stream path reaches the host as an approval request the host can answer"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D38, found while verifying wl#1180). Nothing has been done. The measured finding, verbatim: In `--json-stream` mode an egress consent prompt is never sent to the host, and the turn silently stalls for 300s before failing closed with a message blaming a prompt the host was never shown. `ProtocolSink::emit_approval_required` returns early unless `hitl_suspend_enabled` is true — and `with_hitl_suspend(...)` is never called ANYWHERE in the workspace (`grep -rn with_hitl_suspend crates/ --include=*.rs` returns 4 hits: the setter definition and three doc comments; zero invocations, production or test). The json-stream sink at main.rs:5202-5215 is built without it, so the flag is permanently false. Yet bootstrap.rs:3014 installs `BridgeConsentDoorbell` on the session egress policy unconditionally whenever a session egress policy exists. So an `EgressVerdict::Ask` (policy.rs:154) -> `resolve_ask` -> `doorbell.ask()` -> `sink.emit_approval_required(...)` emits NOTHING, and `rx.await` (bridge_doorbell.rs:94) blocks until the reaper cancels it at `DEFAULT_APPROVAL_TTL` = 300s (approval.rs:40), yielding `ConsentDecision::No` -> `EgressDecision::Deny { reason: 'Egress to `host` was declined at the consent prompt...' }`. The doorbell's own comment at bridge_doorbell.rs:88-91 asserts 'this doorbell is only installed where a real surface exists' — on the json-stream path that premise is false. `emit_suspend` (protocol_sink.rs:1100) and `emit_approval_resume` (protocol_sink.rs:1216) are dead behind the same gate. The TUI is unaffected (tui/engine_bridge.rs:573 sends unconditionally), and ForgeFlow/Crucible are unaffected because engine.rs:17144 and engine.rs:26842 emit `ApprovalRequired` straight on the writer, bypassing the sink gate. `GatingProtocolWriter` (main.rs:4256) does not cover this either — it only synthesizes on `ToolRequest`, and an egress consent has no ToolRequest (its call_id is `egress:<uuid>`)."
  - id: c2
    text: "No path installs BridgeConsentDoorbell where the sink cannot emit -- either with_hitl_suspend is called on the json-stream sink, or the doorbell is not installed there"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D38). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A consent that was never shown is never reported to the user as 'declined at the consent prompt'"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D38). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "A test drives the json-stream sink through an egress Ask and asserts an ApprovalRequired frame is written; shown RED against today's hitl_suspend_enabled gate"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D38). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

In `--json-stream` mode an egress consent prompt is never sent to the host, and the turn silently stalls for 300s before failing closed with a message blaming a prompt the host was never shown. `ProtocolSink::emit_approval_required` returns early unless `hitl_suspend_enabled` is true — and `with_hitl_suspend(...)` is never called ANYWHERE in the workspace (`grep -rn with_hitl_suspend crates/ --include=*.rs` returns 4 hits: the setter definition and three doc comments; zero invocations, production or test). The json-stream sink at main.rs:5202-5215 is built without it, so the flag is permanently false. Yet bootstrap.rs:3014 installs `BridgeConsentDoorbell` on the session egress policy unconditionally whenever a session egress policy exists. So an `EgressVerdict::Ask` (policy.rs:154) -> `resolve_ask` -> `doorbell.ask()` -> `sink.emit_approval_required(...)` emits NOTHING, and `rx.await` (bridge_doorbell.rs:94) blocks until the reaper cancels it at `DEFAULT_APPROVAL_TTL` = 300s (approval.rs:40), yielding `ConsentDecision::No` -> `EgressDecision::Deny { reason: 'Egress to `host` was declined at the consent prompt...' }`. The doorbell's own comment at bridge_doorbell.rs:88-91 asserts 'this doorbell is only installed where a real surface exists' — on the json-stream path that premise is false. `emit_suspend` (protocol_sink.rs:1100) and `emit_approval_resume` (protocol_sink.rs:1216) are dead behind the same gate. The TUI is unaffected (tui/engine_bridge.rs:573 sends unconditionally), and ForgeFlow/Crucible are unaffected because engine.rs:17144 and engine.rs:26842 emit `ApprovalRequired` straight on the writer, bypassing the sink gate. `GatingProtocolWriter` (main.rs:4256) does not cover this either — it only synthesizes on `ToolRequest`, and an egress consent has no ToolRequest (its call_id is `egress:<uuid>`).

**Where.** crates/wcore-agent/src/output/protocol_sink.rs:1085 (the gate) + crates/wcore-cli/src/main.rs:5202 (sink built without with_hitl_suspend) + crates/wcore-agent/src/bootstrap.rs:3014 (doorbell installed unconditionally) + crates/wcore-agent/src/egress/bridge_doorbell.rs:88-94

**Why it matters.** User-visible on the Desktop host: a five-minute dead stall with no modal, then a false 'declined at the consent prompt' error. It also makes the seam #1180 just graded unreachable in production via the egress path on json-stream — the host can never be handed a resume_token to echo back, so `handle_approval_resume` can never resolve an egress consent there. That is the same shape as the bug #1180 was filed for, one layer up. No issue exists: `gh search issues --repo FerroxLabs/wayland hitl_suspend` returns nothing, and the 'egress consent' search returns only #1180/#583/#569/#497/#568, none of which is this.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
