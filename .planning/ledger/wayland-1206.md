---
issue: 1206
repo: FerroxLabs/wayland
kind: defect
title: "The #1166 absolute floor reports PartialMiss{TtlExpiry} and writes a durable 'expired' invalidation on a healthy turn"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A turn whose cache_read is unchanged from the previous turn while total input grows is not attributed to TtlExpiry"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D24, found while verifying wayland#1166). Nothing has been done. The measured finding, verbatim: The #1166 absolute floor reports `PartialMiss { cause: TtlExpiry }` — and writes a durable `expired` invalidation into the cache ledger — on a turn where the cache worked perfectly. The floor tests `cache_read / total_input < 0.3`, so any warm turn whose NEW input dwarfs the cached prefix trips it even when the entire prefix was read back verbatim. Reproduced, not modelled: driving the real module (verbatim copy, no edits) through 3 healthy turns at cache_read=40,000 / input=500, then one turn at cache_read=40,000 (same full prefix, still read back) / input=150,000, printed `PROBE diag=PartialMiss { hit_rate: 0.2105, cause: TtlExpiry }` and `PROBE alert=Some(CacheHealthAlert { round_trip: 4, input_tokens: 190000, cache_read_tokens: 40000, ratio: 0.2105, cause: TtlExpiry })`. Nothing was invalidated on that turn: prev cache_read == current cache_read, so before #1166 this returned Healthy (drop_pct = 1 - 40000/40000 = 0.0, not > 0.05). The false verdict is durable and on by default: engine.rs:17927 does `cause_of_diagnostic(diagnostic)` → `InvalidationCause::Expired` into the ledger, and `cache_ledger::recording_enabled()` (:886-894) returns true when the env var is unset. So `wayland cache` will report invalidations, attributed to the server's TTL, that never happened."
  - id: c2
    text: "No InvalidationCause::Expired is written to the cache ledger for such a turn"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D24). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test drives the measured sequence -- three turns at cache_read=40,000/input=500, then one at cache_read=40,000/input=150,000 -- and asserts Healthy; shown RED against today's floor"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D24). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "genuinely_healthy_trace_stays_healthy and warm_session_healthy_ratio_does_not_warn stay green, and at least one new case sits at the boundary rather than far from it"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D24). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The #1166 absolute floor reports `PartialMiss { cause: TtlExpiry }` — and writes a durable `expired` invalidation into the cache ledger — on a turn where the cache worked perfectly. The floor tests `cache_read / total_input < 0.3`, so any warm turn whose NEW input dwarfs the cached prefix trips it even when the entire prefix was read back verbatim. Reproduced, not modelled: driving the real module (verbatim copy, no edits) through 3 healthy turns at cache_read=40,000 / input=500, then one turn at cache_read=40,000 (same full prefix, still read back) / input=150,000, printed `PROBE diag=PartialMiss { hit_rate: 0.2105, cause: TtlExpiry }` and `PROBE alert=Some(CacheHealthAlert { round_trip: 4, input_tokens: 190000, cache_read_tokens: 40000, ratio: 0.2105, cause: TtlExpiry })`. Nothing was invalidated on that turn: prev cache_read == current cache_read, so before #1166 this returned Healthy (drop_pct = 1 - 40000/40000 = 0.0, not > 0.05). The false verdict is durable and on by default: engine.rs:17927 does `cause_of_diagnostic(diagnostic)` → `InvalidationCause::Expired` into the ledger, and `cache_ledger::recording_enabled()` (:886-894) returns true when the env var is unset. So `wayland cache` will report invalidations, attributed to the server's TTL, that never happened.

**Where.** crates/wcore-agent/src/cache_diagnostics.rs:325-332 (the floor) feeding crates/wcore-agent/src/cache_ledger.rs:287-294 (cause_of_diagnostic) and crates/wcore-agent/src/engine.rs:17927 (ledger write)

**Why it matters.** This is the ticket's own Defect 4 re-entering through the fix. #1166's complaint was that TtlExpiry is a fall-through that blames the server for client-side causes; the fix widened the set of turns that reach that fall-through without narrowing it, so the tool built to diagnose #559 now manufactures confident `expired` findings on healthy sessions. No test covers the shape — genuinely_healthy_trace_stays_healthy uses a ~0.99 ratio and warm_session_healthy_ratio_does_not_warn uses 0.8, so both sit far from the boundary. A cheap discriminator exists and is unused: `cache_read` being flat-and-equal to the previous turn while total input grows is prefix reuse, not invalidation.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
