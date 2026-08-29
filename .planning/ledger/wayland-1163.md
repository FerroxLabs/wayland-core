---
issue: 1163
repo: FerroxLabs/wayland
kind: defect
title: "Cache ledger reports a negative saving against a fabricated zero counterfactual, graded cost_truth=priced"
status: closed
last_verified_commit: 00081ad24
criteria:
  - id: c1
    text: "An unpriceable model reports its saving as unknown instead of manufacturing a zero baseline"
    state: met
    evidence: "symbol:crates/wcore-agent/src/cache_ledger.rs::cache_saving_usd"
    owner: core
    note: "uncached_equivalent_usd is Option<f64>. RE-ANCHORED 2026-08-29 off the bare line number onto LedgerSummary::cache_saving_usd, the accessor that returns None when the counterfactual is unknown; a struct FIELD is not a resolvable symbol for the gate, and a line number drifts. HOLDS FOR A SESSION THIS BUILD RECORDED; for a session an OLDER build recorded it did not, and that is c4."
  - id: c2
    text: "The RENDERED verdict says unknown, not a negative number"
    state: met
    evidence: "test:crates/wcore-cli/tests/cache_ledger_cli.rs::an_unpriced_counterfactual_renders_unknown_instead_of_a_negative_saving"
    owner: core
  - id: c3
    text: "Saving truthfulness is reported separately rather than by degrading cost_truth"
    state: met
    evidence: "symbol:crates/wcore-agent/src/cache_ledger.rs::saving_truth"
    owner: core
    note: "sub-ask (c) proposed degrading cost_truth; declined, because that would be a second false claim and would flip `cache verify` to exit 7 for every unlisted-model session"
  - id: c4
    text: "Reading back a ledger an OLDER build wrote does not reproduce the fabricated zero"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_ledger.rs::a_legacy_zero_counterfactual_does_not_read_back_as_a_priced_zero"
    owner: core
    note: "ADDED 2026-08-29 after re-grading c1/c2/c3 at HEAD. The three shipped criteria all hold for a session THIS build recorded, and all three fail for one an older build recorded -- which is the operator in the ticket, who filed it off a persisted v0.13.9 session and would run `cache report` over the ledger directory they already have. TurnSample::uncached_equivalent_usd changed meaning in place, from `f64` (v0.13.9 wrote 0.0 exactly when nothing could price it) to `Option<f64>` (Some(0.0) is a genuine priced zero), and LEDGER_SCHEMA was left at 1 -- against the module`s own rule three lines above it, `Bumped when a field`s meaning changes`. `#[serde(default)]` therefore laundered every legacy 0.0 into Some(0.0). REPRODUCED before it was fixed, as a test rather than a model: a_legacy_zero_counterfactual_does_not_read_back_as_a_priced_zero was written first and RED at e7144c30a. FIX: LEDGER_SCHEMA = 2, and `load` migrates a v1 file in memory (migrate_v1_counterfactual maps a v1 zero back to None) instead of refusing it -- refusing would drop the operator`s existing ledgers out of `cache list` silently, which is the same shape of harm one level over. The migration is read-only; the file on disk keeps its own version until something writes it. A v1 turn whose counterfactual was a genuine priced zero is demoted to unknown, and that is the safe direction: unknown renders as unknown, a false zero renders as a confident negative number. CONTROLS: a_legacy_priced_counterfactual_survives_the_migration proves the migration does not blanket-erase counterfactuals (a v1 row at 0.05 still reports a +0.03 saving), a_new_ledger_is_stamped_with_the_schema_that_carries_the_new_meaning pins the bump, and the pre-existing load_refuses_unknown_schema still refuses LEDGER_SCHEMA + 1. RED ARM (2026-08-30): LEDGER_SCHEMA put back to 1 and the v1 arm removed from `load` (restoring the original `if ledger.schema != LEDGER_SCHEMA` refusal) -- both mutations on executable code, quoted in the diff as a const initialiser and a match. 24 tests -> 22 passed, 2 failed. VERBATIM: `thread 'cache_ledger::tests::a_legacy_zero_counterfactual_does_not_read_back_as_a_priced_zero' panicked at crates/wcore-agent/src/cache_ledger.rs:1499:9: assertion `left == right` failed: v1 wrote 0.0 to mean `nothing could price this`; decoding it as Some(0.0) is the fabricated baseline #1163 is about / left: Some(0.0) / right: None`; and a_new_ledger_is_stamped_with_the_schema_that_carries_the_new_meaning: `uncached_equivalent_usd changed meaning in place; the module's own rule is that the version is bumped when it does`. Restored + touched: 24/24 pass. MEASURED END TO END AT THE CLI as well as in the unit test, because the ticket was filed off a `cache report` run and a unit test is not that surface. A v1 ledger was hand-written in the shape v0.13.9 wrote (schema 1, flux-router/flux-reasoning, cost_source=provider_reported, uncached_equivalent_usd: 0.0) into WAYLAND_HOME, and all three verbs were run against the built binary, on both arms of the same mutation, with the binary md5 recorded each time. RED (LEDGER_SCHEMA=1, no v1 migration; md5 afd594f65b586b49b92c1e04cc92a098): `F23_CACHE=cost usd=0.061389 uncached_equivalent_usd=0.000000 saving_usd=-0.061389 saving_ratio=unknown cost_truth=priced saving_truth=priced counterfactual_unpriced_round_trips=0` with NO saving_warning line; `F23_CACHE=total ... uncached_equivalent_usd=0.000000 cost_truth=priced`; `F23_CACHE=verify trustworthy=true cost_truth=priced saving_truth=priced`, exit 0. That first line is the ticket`s own report, and `saving_truth=priced` is worse than the ticket: it certifies the fabricated saving. FIXED (md5 7e15a56c8bae56c0fee911401e96ee55, and the RESTORED build hashes identically to the fixed one, so the two arms are provably different binaries and the restore is byte-exact): `F23_CACHE=cost usd=0.061389 uncached_equivalent_usd=unknown saving_usd=unknown saving_ratio=unknown cost_truth=priced saving_truth=unpriced counterfactual_unpriced_round_trips=1` plus `F23_CACHE=saving_warning text=no_catalog_rate_for_the_uncached_counterfactual`, and `cache list`'s store total also reads `uncached_equivalent_usd=unknown`. CLASS: every read of a ledger in the product goes through `cache_ledger::load` -- `list` calls it per file and `latest` calls `list`, and wcore-cli imports exactly those three. One reader, one migration point, all three CLI verbs covered by the run above. FIRST ATTEMPT AT THIS PROOF READ BACK sessions=0 AND PROVED NOTHING: the hand-written ledger was refused for an unknown `retention` variant and `list` skips a malformed file silently, so the absence of a negative saving was the absence of any ledger. Corrected and re-run; the numbers above are from the corrected run."
---

Closed in v0.13.10, with one deliberate deviation from the ticket recorded
above rather than quietly taken.

The ledger priced the counterfactual at zero when the model was not in the
price table, then reported the real spend as a NEGATIVE saving against it.
The counterfactual is now optional and an unknown one renders as unknown.
The ticket's third sub-ask — degrade `cost_truth` — was answered with a new
`saving_truth()` instead; degrading `cost_truth` would have made a second
false claim and broken `cache verify` for every unlisted model.

Re-graded at e7144c30a on 2026-08-29. c1-c3 hold for a session this build
recorded. They did not hold for one an older build recorded: the field
changed meaning without a schema bump, so `serde(default)` read every legacy
`0.0` as a priced zero and the ticket's own report came back verbatim on the
fixed binary — now additionally graded `saving_truth=priced`, a confidence the
pre-fix build never claimed. c4 is that gap, reproduced first and then closed
by bumping the schema and migrating v1 on read.
