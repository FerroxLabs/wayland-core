---
issue: 863
repo: FerroxLabs/wayland
title: "CONTRACT: Loop ownership between Anvil (wayland-core) and Elevation (Flux) — anti-collision invariants"
status: open
last_verified_commit: cfa89a9c
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
    note: "Flux replied that F1 holds with flux-auto elevated only via explicit per-request opt-in, but core has no way to verify a server-side deployment and later recorded F1 as unconfirmed"
  - id: c4
    text: "F3 server half: requests carrying loop_owner or a client nonce bypass or vary the Flux semantic cache"
    state: blocked
    owner: flux
    note: "Flux reports this shipped; the behaviour lives entirely on their side and core cannot observe a cache bypass from the client"
  - id: c5
    text: "F4: the bandit routes loop_owner requests to a tool-calling-capable arm, or a flux-agentic alias with that guarantee exists"
    state: blocked
    owner: flux
    note: "Flux deferred F4 explicitly and disclosed it; Elevation hard-skips tool turns so there is no collision exposure, but the routing floor is still absent"
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
