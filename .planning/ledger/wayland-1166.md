---
issue: 1166
repo: FerroxLabs/wayland
kind: defect
title: "CacheBreakDetector reports Healthy with 0 causes on a 3% hit ratio: a flat cache_read never reaches attribute_cause, and messages are never hashed"
status: closed
last_verified_commit: 3262536a
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
    note: "Re-verified at 43848f75. The message hashing that lets a changed prefix be named with its first divergent index feeds record_request at engine.rs:13213 (formerly cited as :13108, which had drifted)."
  - id: c4
    text: "The snapshot describes the request actually dispatched — taken after the tier swap and transient injections"
    state: met
    evidence: "file:crates/wcore-agent/src/engine.rs:13213"
    owner: core
    note: "RE-ANCHORED 2026-08-29: the old anchor engine.rs:13108 still resolved but had drifted onto an unrelated max_tokens sizing block. The record_request site is now at :13213, and the comment immediately above it at :13207-13212 states that the snapshot is taken after the tier swap and the transient tail injections, which is the claim."
  - id: c5
    text: "The detector can still report Healthy when the cache genuinely is — a positive control that passes in every mutation arm"
    state: met
    evidence: "file:crates/wcore-agent/src/cache_diagnostics.rs:346"
    owner: core
    note: "healthy-trace control; ModelChanged arm is what distinguishes it from a detector stuck on unhealthy"
  - id: c6
    text: "The absolute floor does not report a break on a turn whose cached prefix was served in full"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_diagnostics.rs::a_large_new_paste_does_not_fake_a_ttl_expiry"
    owner: core
    note: "c1's floor tests cache_read/total_input, so a warm turn carrying a large NEW input trips it even when every cached token came back. REPRODUCED: three healthy turns at cache_read=40,000/input=500 then one at cache_read=40,000/input=150,000 returned `PartialMiss { hit_rate: 0.2105, cause: TtlExpiry }`. Coverage is now measured against the input the PREVIOUS turn processed -- the part that could have been cached -- in both the floor and check_cache_health, so the two halves cannot disagree (the_floor_and_the_health_probe_never_disagree). The #559 leader trace still fires: 192 flat covers none of the previous turn either"
  - id: c7
    text: "No `expired` invalidation is written to the durable ledger on such a turn"
    state: met
    evidence: "test:crates/wcore-agent/tests/cache_ledger_engine_test.rs::a_large_new_paste_is_not_recorded_as_a_server_side_expiry"
    owner: core
    note: "graded at the FILE, not at the detector: cause_of_diagnostic feeds engine.rs and cache_ledger::recording_enabled() is on by default, so the false verdict was durable and `wayland-core cache report` would show it for ever. This is the ticket's own Defect 4 re-entering through its fix"
  - id: c8
    text: "The verdict reaches the user at the SHIPPED default configuration"
    state: met
    evidence: "test:crates/wcore-agent/tests/cache_ledger_engine_test.rs::a_dead_prompt_cache_tells_the_user_once_at_the_default_config"
    owner: core
    note: "the ticket's Defect 5 (`off by default`), which had no criterion at all. The one ungated surface was a tracing::warn!, and with RUST_LOG unset the CLI routes everything below ERROR to a log file (TUI mode: file only), so it reached nobody. A warm session whose prefix is not read back now says so on the OutputSink once per session, ungated. `compact.cache_diagnostics` stays false ON PURPOSE and the reason is written on the field: it gates the per-TURN hit-rate line, which on a healthy session is noise every turn. Control: a_healthy_session_is_told_nothing_about_its_cache"
---

Closed in v0.13.10. The detector graded a 3% hit ratio as Healthy with zero
causes: a `cache_read` that was flat rather than falling never reached
`attribute_cause` at all, and the messages array was never hashed, so a
changed prefix could not be named even when it was the cause.

All four graded defects are closed and the snapshot moved to after the tier
swap, so it describes the request that actually went on the wire. A healthy
control is included deliberately — a detector that can only say "broken" is
as useless as one that can only say "fine".
