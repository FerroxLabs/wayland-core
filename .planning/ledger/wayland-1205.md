---
issue: 1205
repo: FerroxLabs/wayland
kind: defect
title: "A cache ledger written by v0.13.9 reproduces #1163 verbatim on the fixed build, and the fix now certifies the fabricated saving as trustworthy"
status: open
last_verified_commit: 7a7cf1f6
criteria:
  - id: c1
    text: "LEDGER_SCHEMA is bumped, and a v1 row's uncached_equivalent_usd: 0.0 no longer decodes as a genuine priced zero"
    state: met
    evidence: "symbol:crates/wcore-agent/src/cache_ledger.rs::migrate_v1_counterfactuals"
    owner: core
    note: "LEDGER_SCHEMA is 2. `load` migrates a v1 file rather than refusing it -- its billed cost_usd and cost_source did not change meaning and are worth showing -- but every v1 uncached_equivalent_usd becomes None. Dropped wholesale, not only the zeros: v0.13.9 wrote the field unconditionally as a bare f64, so on disk it conflates a catalog price, a provider-family CEILING and 0.0 meaning nothing could price it, and no v1 row carries the provenance to tell them apart. The ticket sanctions exactly this (`or refuse v1 rows for the saving`). A schema this build has never seen is still refused: test the_migration_does_not_make_load_accept_every_schema."
  - id: c2
    text: "cache report over a v0.13.9-format ledger no longer prints saving_truth=priced with a negative saving; the run output is quoted"
    state: met
    evidence: "test:crates/wcore-cli/tests/cache_ledger_cli.rs::a_v0_13_9_ledger_no_longer_reports_a_negative_saving_as_priced"
    owner: core
    note: "Run quoted, shipped debug binary on hetzner over the v0.13.9-format ledger the ticket measured. BEFORE: `F23_CACHE=cost usd=0.061389 uncached_equivalent_usd=0.000000 saving_usd=-0.061389 saving_ratio=unknown cost_truth=priced saving_truth=priced counterfactual_unpriced_round_trips=0` with no saving_warning. AFTER: `F23_CACHE=cost usd=0.061389 uncached_equivalent_usd=unknown saving_usd=unknown saving_ratio=unknown cost_truth=priced saving_truth=unpriced counterfactual_unpriced_round_trips=1 provider_reported_round_trips=1 catalog_priced_round_trips=0 estimated_round_trips=0 unpriced_round_trips=0` plus `F23_CACHE=saving_warning text=no_catalog_rate_for_the_uncached_counterfactual saving_truth=unpriced counterfactual_unpriced_round_trips=1`. The billed figure is unchanged, which is the point: provider-reported spend is still spend."
  - id: c3
    text: "cache list's store total does not sum a legacy 0.0 into the counterfactual, and cache verify does not return trustworthy=true for it"
    state: met
    evidence: "test:crates/wcore-cli/tests/cache_ledger_cli.rs::a_legacy_ledger_is_not_summed_into_the_store_total_and_does_not_pass_verify"
    owner: core
    note: "Both halves, run quoted. `cache list` over a store holding one legacy and one current session asserts `F23_CACHE=total sessions=2 ... uncached_equivalent_usd=unknown cost_truth=priced` -- the legacy zero is not summed in, it makes the store counterfactual unknown. (An earlier revision of this note quoted `sessions=1` here; that figure was copied from a single-session manual run and did not match the two-session store the sentence describes. The value above is the one the named test asserts.) `cache verify` exits 7 with `F23_CACHE=verify trustworthy=false cost_truth=priced saving_truth=unpriced legacy_schema=1 ...`; trustworthy is now `cost_truth().is_trustworthy() && !ledger.is_migrated()`, because a file whose field meanings were reinterpreted on read must be reported and not certified. The test carries a control: the current-schema session beside it still verifies `legacy_schema=none` at exit 0, so verify has not simply been broken for everything. Shown RED: dropping the `&& !ledger.is_migrated()` clause gives `assertion `left == right` failed: a migrated ledger is reported, never certified: left: "true" / right: "false"`."
  - id: c4
    text: "A test reads a fixture ledger in the v1 on-disk shape and asserts the verdict; shown RED against today's #[serde(default)] decode"
    state: met
    evidence: "test:crates/wcore-agent/src/cache_ledger.rs::a_v1_row_does_not_decode_its_zero_counterfactual_as_a_priced_zero"
    owner: core
    note: "Reads a hand-written fixture in the v1 ON-DISK shape (schema 1, uncached_equivalent_usd a bare 0.0, cost_source provider_reported) rather than one serialized from the current struct -- the whole point is that the struct changed underneath it. Shown RED against the #[serde(default)] decode: with the turns loop in migrate_v1_counterfactuals removed, `assertion `left == right` failed: the v1 zero is not a price / left: Some(0.0) / right: None`."
---

Reading back any cache ledger written by v0.13.9 or earlier reproduces #1163 verbatim on the FIXED build, and adds a second false claim. `TurnSample::uncached_equivalent_usd` changed meaning from `f64` (0.0 = 'nothing could price it', v0.13.9 cache_ledger.rs:153, engine.rs:17129 `uncached_equivalent_usd: uncached.usd`) to `Option<f64>` (Some(0.0) = 'a genuine priced zero'), but `LEDGER_SCHEMA` was left at 1 — even though the module's own doc at cache_ledger.rs:72 says it is 'Bumped when a field's meaning changes'. `#[serde(default)]` therefore decodes every legacy `0.0` as `Some(0.0)`. Measured on hetzner with the freshly built binary sweep-targets/1163/debug/wayland-core against a hand-written legacy-format ledger (flux-router/flux-reasoning, cost_source=provider_reported, uncached_equivalent_usd: 0.0 — the exact shape v0.13.9 wrote, confirmed against `git show v0.13.9`): cache report → `F23_CACHE=cost usd=0.061389 uncached_equivalent_usd=0.000000 saving_usd=-0.061389 saving_ratio=unknown cost_truth=priced saving_truth=priced counterfactual_unpriced_round_trips=0` and NO saving_warning line. cache list → `F23_CACHE=total ... uncached_equivalent_usd=0.000000 cost_truth=priced`. cache verify → `trustworthy=true cost_truth=priced saving_truth=priced`, exit 0. That first line is character-for-character the report in the ticket body. The new `saving_truth=priced` is a regression in kind: the fix's own verdict now certifies the fabricated saving as trustworthy, which the pre-fix build never claimed.

**Where.** crates/wcore-agent/src/cache_ledger.rs:72 (LEDGER_SCHEMA = 1, not bumped) and :173-174 (`#[serde(default, skip_serializing_if)] pub uncached_equivalent_usd: Option<f64>`); rendered at crates/wcore-cli/src/cache_cmd.rs:619-645. Doc comment at cache_ledger.rs:170-172 acknowledges the decode ('a 0.0 read back from a ledger an older build wrote decodes as Some(0.0), which is what that build meant by it') — but that is not what the old build meant by it: v0.13.9 wrote 0.0 precisely when nothing could price the model.

**Why it matters.** The cache ledger is a durable on-disk artifact whose stated purpose (cache_ledger.rs:22-24) is that a separate process can read it back without an engine. #1163 was itself filed off a persisted 0.13.9 session. So the operator who reported this bug, on upgrading, runs `cache report` over the ledger directory they already have and sees the identical negative saving — now graded `saving_truth=priced`. One legacy session also poisons `cache list`'s store total, since StoreTotals sums Some(0.0) happily. Fix is bounded: bump LEDGER_SCHEMA to 2 and have the v1 reader map `uncached_equivalent_usd: 0.0` to None (or refuse v1 rows for the saving), which is what the module's own schema rule already prescribes.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
