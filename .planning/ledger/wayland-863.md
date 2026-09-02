---
issue: 863
repo: FerroxLabs/wayland
kind: defect
title: "CONTRACT: Loop ownership between Anvil (wayland-core) and Elevation (Flux) — anti-collision invariants"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "F2 client half: core marks driver-seat requests with loop_owner on the wire, on a concrete model id"
    state: met
    evidence: "test:crates/wcore-providers/tests/flux_loop_provenance.rs::chat_wire_carries_loop_owner_on_a_concrete_model_id"
    owner: core
    note: "emission gates on the endpoint via ProviderCompat.flux_loop_provenance, not on the alias, because F2 says Flux honours loop_owner regardless of alias"
  - id: c2
    text: "F2 detector half: an unrequested elevation echo on a loop-owned turn is a runtime hard fault, not a log line"
    state: met
    evidence: "test:crates/wcore-agent/tests/flux_loop_collision_engine_test.rs::elevation_on_a_loop_owned_turn_is_a_hard_fault"
    owner: core
    note: "cascade and a missing header are explicitly excluded from the fault, which the sibling tests in the same file pin"
  - id: c3
    text: "F1 confirmed for the current deployment: Elevation is unreachable by default from flux-fast, flux-standard, flux-reasoning and flux-auto"
    state: blocked
    owner: flux
    handoff: "FerroxLabs/wayland#1227"
    note: "AUDITED 2026-08-29. #863 is itself the two-party contract, so a handoff naming #863 would have been self-referential and worthless; the carrier is #1183, a SEPARATE flux-owned ticket the core lane filed for exactly F1/F3/F4, open and needs:flux. Flux replied that F1 holds with flux-auto elevated only by explicit per-request opt-in, and core later recorded F1 as unconfirmed because it cannot verify a server-side deployment. #1183 asks for the observable that makes it checkable: a per-alias capability endpoint declaring off/opt-in/always, or, if that is refused, a dated versioned statement of deployed config that core pins in docs/providers.md and re-checks each release. The reason a bare probe is not accepted is stated there -- a probe that never elevates is consistent with 'elevation is off' AND with 'elevation is on but rare' HANDOFF TARGET RECONCILED ON MERGE: this criterion was decomposed twice on 2026-08-29, by two lanes that could not see each other. The audit lane pointed it at FerroxLabs/wayland#1183, which already existed and already carries the work; the decomposition lane filed FerroxLabs/wayland#1227 and that is the ticket named above, because it is scoped to this criterion. BOTH ARE OPEN AND THEY OVERLAP -- both carry F1/F3/F4 of this two-party contract to the flux lane; #1183 was filed by the core lane for exactly these three. Core does not close issues, so this is recorded rather than acted on: a maintainer should dedupe the pair, and whichever survives is the carrier. The audit evidence in this note was gathered against FerroxLabs/wayland#1183 and applies to either."
  - id: c4
    text: "F3 server half: requests carrying loop_owner or a client nonce bypass or vary the Flux semantic cache"
    state: blocked
    owner: flux
    handoff: "FerroxLabs/wayland#1227"
    note: "AUDITED 2026-08-29; carried by #1183, the flux-owned split of this contract, open and needs:flux. Flux reports F3 shipped and core cannot observe a cache bypass from the client, which is why the ask is an observable rather than a re-test: X-Flux-Cache: hit|miss|bypass (and/or response.flux.cache), with bypass distinct from miss. The ticket also demands the POSITIVE CONTROL, which is the part that makes it honest -- the same prompt sent twice WITHOUT loop_owner must read miss then hit, because a header that always says bypass and a cache that is simply switched off are indistinguishable, and so are 'we bypass for loop_owner' and 'we bypass for everything' HANDOFF TARGET RECONCILED ON MERGE: this criterion was decomposed twice on 2026-08-29, by two lanes that could not see each other. The audit lane pointed it at FerroxLabs/wayland#1183, which already existed and already carries the work; the decomposition lane filed FerroxLabs/wayland#1227 and that is the ticket named above, because it is scoped to this criterion. BOTH ARE OPEN AND THEY OVERLAP -- both carry F1/F3/F4 of this two-party contract to the flux lane; #1183 was filed by the core lane for exactly these three. Core does not close issues, so this is recorded rather than acted on: a maintainer should dedupe the pair, and whichever survives is the carrier. The audit evidence in this note was gathered against FerroxLabs/wayland#1183 and applies to either."
  - id: c5
    text: "F4: the bandit routes loop_owner requests to a tool-calling-capable arm, or a flux-agentic alias with that guarantee exists"
    state: blocked
    owner: flux
    handoff: "FerroxLabs/wayland#1227"
    note: "AUDITED 2026-08-29; carried by #1183, open and needs:flux. Flux deferred F4 explicitly and disclosed it, and core accepted that there is no collision exposure today because Elevation hard-skips tool turns -- so this is the routing FLOOR only and is the lowest-urgency of the three. #1183 offers two acceptable shapes: a flux-agentic alias whose documented guarantee is that every served arm supports tool calling, or a published tool-capable arm set that core checks x-flux-routed-model against; either way the floor must state what happens when no tool-capable arm is available rather than leaving it to observation HANDOFF TARGET RECONCILED ON MERGE: this criterion was decomposed twice on 2026-08-29, by two lanes that could not see each other. The audit lane pointed it at FerroxLabs/wayland#1183, which already existed and already carries the work; the decomposition lane filed FerroxLabs/wayland#1227 and that is the ticket named above, because it is scoped to this criterion. BOTH ARE OPEN AND THEY OVERLAP -- both carry F1/F3/F4 of this two-party contract to the flux lane; #1183 was filed by the core lane for exactly these three. Core does not close issues, so this is recorded rather than acted on: a maintainer should dedupe the pair, and whichever survives is the carrier. The audit evidence in this note was gathered against FerroxLabs/wayland#1183 and applies to either."
---

This is a two-party contract between wayland-core's client-side Anvil climb and
Flux's server-side Elevation ladder. The one rule is exactly one ladder per
task: a request path must never run both. F1 through F6 are invariants asked of
Flux; core's obligation is the client half of the handshake.

Core's half is built and in main. Requests core owns carry
`metadata.loop_owner` and `X-Flux-Loop-Owner` on the OpenAI chat, Responses and
Anthropic paths (Anthropic header-only by design, since its `metadata` accepts
only `user_id`), the `x-flux-loop-engaged` echo is parsed back into
`ProviderMeta`, and an `elevation` echo on a loop-owned turn returns an error
rather than a note. F5 is enforced by the type: `ClientOwned` and `ServerVerify`
are arms of one enum, so the wrong combination is unrepresentable.

The three blocked criteria are the asks that only Flux can satisfy or confirm.
This issue was twice mis-routed to Flux as a whole on a reading of the body, and
twice sat — the body is from core, but core's own half was outstanding and is
now done. It stays open because closing a contract from one end is how the two
ends drift.
