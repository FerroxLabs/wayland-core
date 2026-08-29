---
issue: 1163
repo: FerroxLabs/wayland
kind: defect
title: "Cache ledger reports a negative saving against a fabricated zero counterfactual, graded cost_truth=priced"
status: closed
last_verified_commit: 43848f75
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
---

Closed in v0.13.10, with one deliberate deviation from the ticket recorded
above rather than quietly taken.

The ledger priced the counterfactual at zero when the model was not in the
price table, then reported the real spend as a NEGATIVE saving against it.
The counterfactual is now optional and an unknown one renders as unknown.
The ticket's third sub-ask — degrade `cost_truth` — was answered with a new
`saving_truth()` instead; degrading `cost_truth` would have made a second
false claim and broken `cache verify` for every unlisted model.
