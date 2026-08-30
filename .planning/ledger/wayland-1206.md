---
issue: 1206
repo: FerroxLabs/wayland
kind: defect
title: "The #1166 absolute floor reports PartialMiss{TtlExpiry} and writes a durable 'expired' invalidation on a healthy turn"
status: open
last_verified_commit: 7a7cf1f6
criteria:
  - id: c1
    text: "A turn whose cache_read is unchanged from the previous turn while total input grows is not attributed to TtlExpiry"
    state: met
    evidence: "symbol:crates/wcore-agent/src/cache_diagnostics.rs::attribute_break"
    owner: core
    note: "Closed as a CLASS, at the one decision point every break arm now routes through. `attribute_break` runs `attribute_cause` first and rejects ONLY the `TtlExpiry` fall-through, and only when this turn's `cache_read` is unchanged from the previous turn's while total input grows -- a named cause (model / system / tools / a mutated message) is evidence we produced ourselves and still outranks the arithmetic, so the whole #1166 attribution is untouched. There were TWO reachable instances of the shape and both are closed: the absolute floor (test a_prefix_read_back_whole_under_a_flood_of_new_input_is_not_ttl_expiry) and the FULL-MISS arm, where prev.cache_read == 0 with prev cache_creation > 0 and this turn also reads 0 (test a_full_miss_whose_cache_read_never_moved_is_not_the_servers_fault). The third arm, the >5% drop, is routed through the same call for uniformity but is unreachable by the shape by construction: it requires cache_read to have FALLEN by more than 5%, and the shape is cache_read unchanged. The alert half is the fourth site and reads the diagnostic half's verdict rather than re-deriving one -- check_cache_health runs AFTER prev_stats has rotated, so it cannot make the comparison itself, which is why the measured turn produced a PartialMiss{TtlExpiry} AND an alert carrying TtlExpiry. The two instances are guarded INDEPENDENTLY, by two different mutations: reverting the FULL-MISS arm alone to a bare attribute_cause reddens only a_full_miss_whose_cache_read_never_moved_is_not_the_servers_fault (`left: TtlExpiry / right: Unattributed`), so a fix that had closed the floor alone would not have passed here."
  - id: c2
    text: "No InvalidationCause::Expired is written to the cache ledger for such a turn"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_ledger.rs::a_prefix_reused_under_growing_input_writes_no_expired_invalidation"
    owner: core
    note: "Drives the measured sequence through `cause_of_diagnostic` -- the exact call `AgentEngine::record_cache_ledger_turn` makes -- and asserts it returns None, so no invalidation of any kind is recorded, least of all Expired. Shown RED: reverting the floor arm alone to a bare attribute_cause gives `assertion `left == right` failed: nothing was invalidated, so nothing may be recorded as one: PartialMiss { hit_rate: 0.21052631578947367, cause: TtlExpiry } / left: Some(Expired) / right: None`. Where the shape leaves a genuine finding (a cache that was never carrying the session), the new `CacheBreakCause::Unattributed` publishes as `InvalidationCause::Unknown`, never `expired`: test an_unattributed_break_is_published_as_unknown_not_expired."
  - id: c3
    text: "A test drives the measured sequence -- three turns at cache_read=40,000/input=500, then one at cache_read=40,000/input=150,000 -- and asserts Healthy; shown RED against today's floor"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_diagnostics.rs::a_prefix_read_back_whole_under_a_flood_of_new_input_is_not_ttl_expiry"
    owner: core
    note: "The measured sequence verbatim -- three turns at cache_read=40,000/input=500, then one at cache_read=40,000/input=150,000 -- asserting Healthy, and additionally asserting check_cache_health returns None so the alert half cannot certify what the diagnostic half refused. Shown RED: reverting the floor arm to a bare attribute_cause gives `the whole prefix came back; got PartialMiss { hit_rate: 0.21052631578947367, cause: TtlExpiry }`. The alert assertion has its own mutation: making check_cache_health treat a refuted turn as TtlExpiry again gives `left: Some(CacheHealthAlert { round_trip: 4, input_tokens: 190000, cache_read_tokens: 40000, ratio: 0.21052631578947367, cause: TtlExpiry }) / right: None`. A third, separate mutation -- dropping refute_ttl's prev-hit-rate tie-break -- reddens the dead-flat #559 case instead (`expected the floor to still fire, got Healthy { hit_rate: 0.020887728459530026 }`), which is what stops this fix from laundering a genuinely dead cache into Healthy."
  - id: c4
    text: "genuinely_healthy_trace_stays_healthy and warm_session_healthy_ratio_does_not_warn stay green, and at least one new case sits at the boundary rather than far from it"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_diagnostics.rs::the_floor_still_bites_one_token_below_the_threshold"
    owner: core
    note: "Both named tests stay green (nextest run quoted in the lane report). The new boundary case runs two warm turns at 2,999/10,000 = 0.2999 and 3,001/10,000 = 0.3001 -- one token either side of CACHE_HEALTH_WARN_RATIO -- against the existing 0.99 and 0.8 cases that sit nowhere near it. Its failing arm deliberately MOVES cache_read so the #1206 refutation is out of the way and the floor itself is what is measured. Shown RED by a mutation nothing else in this lane is sensitive to: moving CACHE_HEALTH_WARN_RATIO to 0.29 gives `one token below the threshold must still be a finding: Healthy { hit_rate: 0.2999 }`."
---

The #1166 absolute floor reports `PartialMiss { cause: TtlExpiry }` — and writes a durable `expired` invalidation into the cache ledger — on a turn where the cache worked perfectly. The floor tests `cache_read / total_input < 0.3`, so any warm turn whose NEW input dwarfs the cached prefix trips it even when the entire prefix was read back verbatim. Reproduced, not modelled: driving the real module (verbatim copy, no edits) through 3 healthy turns at cache_read=40,000 / input=500, then one turn at cache_read=40,000 (same full prefix, still read back) / input=150,000, printed `PROBE diag=PartialMiss { hit_rate: 0.2105, cause: TtlExpiry }` and `PROBE alert=Some(CacheHealthAlert { round_trip: 4, input_tokens: 190000, cache_read_tokens: 40000, ratio: 0.2105, cause: TtlExpiry })`. Nothing was invalidated on that turn: prev cache_read == current cache_read, so before #1166 this returned Healthy (drop_pct = 1 - 40000/40000 = 0.0, not > 0.05). The false verdict is durable and on by default: engine.rs:17927 does `cause_of_diagnostic(diagnostic)` → `InvalidationCause::Expired` into the ledger, and `cache_ledger::recording_enabled()` (:886-894) returns true when the env var is unset. So `wayland cache` will report invalidations, attributed to the server's TTL, that never happened.

**Where.** crates/wcore-agent/src/cache_diagnostics.rs:325-332 (the floor) feeding crates/wcore-agent/src/cache_ledger.rs:287-294 (cause_of_diagnostic) and crates/wcore-agent/src/engine.rs:17927 (ledger write)

**Why it matters.** This is the ticket's own Defect 4 re-entering through the fix. #1166's complaint was that TtlExpiry is a fall-through that blames the server for client-side causes; the fix widened the set of turns that reach that fall-through without narrowing it, so the tool built to diagnose #559 now manufactures confident `expired` findings on healthy sessions. No test covers the shape — genuinely_healthy_trace_stays_healthy uses a ~0.99 ratio and warm_session_healthy_ratio_does_not_warn uses 0.8, so both sit far from the boundary. A cheap discriminator exists and is unused: `cache_read` being flat-and-equal to the previous turn while total input grows is prefix reuse, not invalidation.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
