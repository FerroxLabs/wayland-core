---
issue: 1163
repo: FerroxLabs/wayland
kind: defect
title: "Cache ledger reports a negative saving against a fabricated zero counterfactual, graded cost_truth=priced"
status: closed
last_verified_commit: 3262536a
criteria:
  - id: c1
    text: "An unpriceable model reports its saving as unknown instead of manufacturing a zero baseline"
    state: met
    evidence: "file:crates/wcore-agent/src/cache_ledger.rs:174"
    owner: core
    note: "uncached_equivalent_usd is Option<f64>"
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
    text: "A ledger an earlier build wrote is migrated on read, so its unpriceable counterfactual stays unpriceable instead of decoding as a priced zero"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_ledger.rs::a_v1_ledgers_zero_counterfactual_is_unpriceable_not_a_priced_zero"
    owner: core
    note: "the v0.13.10 fix changed the field's MEANING (f64 0.0 = unpriceable -> Option<f64>, Some(0.0) = a priced zero) without bumping LEDGER_SCHEMA, so serde decoded every legacy 0.0 as a genuine zero and the fixed build reproduced #1163 verbatim off disk. MEASURED on the pre-fix build against a v0.13.9-format ledger: `F23_CACHE=cost usd=0.061389 uncached_equivalent_usd=0.000000 saving_usd=-0.061389 cost_truth=priced saving_truth=priced` with no saving_warning -- character-for-character the ticket body, now additionally certified `saving_truth=priced`, which the pre-#1163 build never claimed. Same command on the fixed build: `uncached_equivalent_usd=unknown saving_usd=unknown saving_truth=unpriced counterfactual_unpriced_round_trips=1` plus the saving_warning line. `cache verify` still exits 0 with trustworthy=true, deliberately: it grades COST (provider-reported here), which is c3's recorded decision"
  - id: c5
    text: "A ledger from the older schema is still readable and still listed, not orphaned by the bump"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_ledger.rs::a_v1_ledger_survives_the_listing_and_is_migrated_in_it"
    owner: core
    note: "`list` skips a file it cannot load, so a naive bump would have silently hidden every pre-existing ledger rather than mis-reporting it. LEDGER_SCHEMA_MIN_READ keeps v1 readable and migrate_to_current_schema brings it forward"
---

Closed in v0.13.10, with one deliberate deviation from the ticket recorded
above rather than quietly taken.

The ledger priced the counterfactual at zero when the model was not in the
price table, then reported the real spend as a NEGATIVE saving against it.
The counterfactual is now optional and an unknown one renders as unknown.
The ticket's third sub-ask — degrade `cost_truth` — was answered with a new
`saving_truth()` instead; degrading `cost_truth` would have made a second
false claim and broken `cache verify` for every unlisted model.
