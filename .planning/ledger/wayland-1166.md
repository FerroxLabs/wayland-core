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
    text: "Ticket Defect 5 -- compact.cache_diagnostics defaulting to false -- is a recorded decision with its reason and its consequence stated, rather than an unexamined default with no criterion covering it"
    state: met
    evidence: "symbol:crates/wcore-config/src/compact.rs::cache_diagnostics_defaults_to_false"
    owner: core
    note: "Added 2026-08-30 by the re-grade lane to satisfy wayland#1207 c2, which exists because this entry had five criteria covering only four of the ticket's five numbered defects -- c5 here is a control, not a defect -- so Defect 5 was absent from the 'all criteria met' reading rather than visible in it. THE DECISION: cache_diagnostics STAYS OFF by default. REASONS. (1) The three surfaces it gates are per-turn `emit_info` lines (crates/wcore-agent/src/engine.rs:15836/15842/15850, all inside `if self.compact_config.cache_diagnostics`), so defaulting them on adds output to every ordinary chat turn; wayland#1150 is open about exactly per-turn overhead, so this would cut against live work. (2) The signal is not lost while off: the ledger records by default (`recording_enabled()` returns true), so cache health stays reconstructable after the fact. CONSEQUENCE, STATED -- and this CORRECTS the severity argument in wayland#1207 itself, which is why the decision is recorded rather than just taken. #1207 reasons that the flag is low-severity because the `cache_health_warn` `tracing::warn!` at engine.rs:15877 'is not gated by the flag and the CLI default filter is EnvFilter::new(\"info\")'. The filter claim is true and was verified -- crates/wcore-cli/src/main.rs:1360, `EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(\"info\"))` -- but the filter decides what reaches the WRITER, and in two of the three subscriber paths the writer is not the terminal. With a TUI log file and `will_enter_tui`, everything goes to the file and the code's own comment says `the alt-screen owns the terminal, so NOTHING may reach stdio -- not even an error`. Headless with a log file tees `non_blocking.and(std::io::stderr.with_max_level(tracing::Level::ERROR))` -- stderr takes ERROR ONLY. Only the third path, where no log file could be opened, writes `warn!` to stderr. So for a default interactive install the cache-health warning lands in a log file, not on the terminal. The accepted state is therefore: with cache_diagnostics off, a default install surfaces cache-health problems in the LOG FILE and the LEDGER and not on the terminal. That is a narrower disclosure than #1207 believed it was accepting, and it is accepted knowingly. The default is pinned by `cache_diagnostics_defaults_to_false` (compact.rs:857 -- #1207 cites :825, which is line drift) against the declaration at compact.rs:275 and the default at :675; `toml_cache_diagnostics_override` at :863 proves an operator can turn it on."
---

Closed in v0.13.10. The detector graded a 3% hit ratio as Healthy with zero
causes: a `cache_read` that was flat rather than falling never reached
`attribute_cause` at all, and the messages array was never hashed, so a
changed prefix could not be named even when it was the cause.

All four graded defects are closed and the snapshot moved to after the tier
swap, so it describes the request that actually went on the wire. A healthy
control is included deliberately — a detector that can only say "broken" is
as useless as one that can only say "fine".
