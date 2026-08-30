---
issue: 1166
repo: FerroxLabs/wayland
kind: defect
title: "CacheBreakDetector reports Healthy with 0 causes on a 3% hit ratio: a flat cache_read never reaches attribute_cause, and messages are never hashed"
status: closed
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A flat cache_read is reported rather than falling through to Healthy — an absolute floor, not only a ratio"
    state: met
    evidence: "file:crates/wcore-agent/src/cache_diagnostics.rs:325:if self.round_trips >= CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS"
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: moved one line, from the middle of the absolute-floor condition to its head, so the fragment names the gate rather than one conjunct of it."
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
    note: "Re-verified at 43848f75. The message hashing that lets a changed prefix be named with its first divergent index feeds record_request at engine.rs:13213 (formerly cited as :13108, which had drifted)."
  - id: c4
    text: "The snapshot describes the request actually dispatched — taken after the tier swap and transient injections"
    state: met
    evidence: "file:crates/wcore-agent/src/engine.rs:13564:self.cache_detector.record_request("
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: the 2026-08-29 re-anchor to :13213 had ITSELF drifted 351 lines onto the closing paren of a smart-routing tracing macro. The record_request site is at :13564 and the comment at :13554-13563 states the claim. This entry has now been silently wrong twice from a bare line number, which is the case #1198 was filed on. RE-ANCHORED 2026-08-29: the old anchor engine.rs:13108 still resolved but had drifted onto an unrelated max_tokens sizing block. The record_request site is now at :13213, and the comment immediately above it at :13207-13212 states that the snapshot is taken after the tier swap and the transient tail injections, which is the claim."
  - id: c5
    text: "The detector can still report Healthy when the cache genuinely is — a positive control that passes in every mutation arm"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_diagnostics.rs::genuinely_healthy_trace_stays_healthy"
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: was cache_diagnostics.rs:346, a line of `attribute_cause`, which is not the positive control this criterion is about. Upgraded to a `test:` token naming the control itself, which the script's own docstring prefers over any positional anchor. healthy-trace control; ModelChanged arm is what distinguishes it from a detector stuck on unhealthy"
---

Closed in v0.13.10. The detector graded a 3% hit ratio as Healthy with zero
causes: a `cache_read` that was flat rather than falling never reached
`attribute_cause` at all, and the messages array was never hashed, so a
changed prefix could not be named even when it was the cause.

All four graded defects are closed and the snapshot moved to after the tier
swap, so it describes the request that actually went on the wire. A healthy
control is included deliberately — a detector that can only say "broken" is
as useless as one that can only say "fine".
