---
issue: 1166
repo: FerroxLabs/wayland
title: "CacheBreakDetector reports Healthy with 0 causes on a 3% hit ratio: a flat cache_read never reaches attribute_cause, and messages are never hashed"
status: closed
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A flat cache_read is reported rather than falling through to Healthy — an absolute floor, not only a ratio"
    state: met
    evidence: "file:crates/wcore-agent/src/cache_diagnostics.rs:326"
    owner: core
  - id: c2
    text: "The alert names a cause instead of defaulting to TTL expiry"
    state: met
    evidence: "symbol:crates/wcore-agent/src/cache_diagnostics.rs::attribute_cause"
    owner: core
  - id: c3
    text: "Request messages are hashed at all, so a changed prefix can be named with its first divergent index"
    state: met
    evidence: "symbol:crates/wcore-agent/src/cache_diagnostics.rs::CacheBreakCause"
    owner: core
    note: "MessagesChanged { first_divergent_index }, fed from &request.messages at engine.rs:13108"
  - id: c4
    text: "The snapshot describes the request actually dispatched — taken after the tier swap and transient injections"
    state: met
    evidence: "file:crates/wcore-agent/src/engine.rs:13108"
    owner: core
  - id: c5
    text: "The detector can still report Healthy when the cache genuinely is — a positive control that passes in every mutation arm"
    state: met
    evidence: "file:crates/wcore-agent/src/cache_diagnostics.rs:346"
    owner: core
    note: "healthy-trace control; ModelChanged arm is what distinguishes it from a detector stuck on unhealthy"
---

Closed in v0.13.10. The detector graded a 3% hit ratio as Healthy with zero
causes: a `cache_read` that was flat rather than falling never reached
`attribute_cause` at all, and the messages array was never hashed, so a
changed prefix could not be named even when it was the cause.

All four graded defects are closed and the snapshot moved to after the tier
swap, so it describes the request that actually went on the wire. A healthy
control is included deliberately — a detector that can only say "broken" is
as useless as one that can only say "fine".
