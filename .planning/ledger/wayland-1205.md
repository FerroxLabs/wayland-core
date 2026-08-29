---
issue: 1205
repo: FerroxLabs/wayland
kind: defect
title: "A cache ledger written by v0.13.9 reproduces #1163 verbatim on the fixed build, and the fix now certifies the fabricated saving as trustworthy"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "LEDGER_SCHEMA is bumped, and a v1 row's uncached_equivalent_usd: 0.0 no longer decodes as a genuine priced zero"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D23, found while verifying wayland#1163). Nothing has been done. The measured finding, verbatim: Reading back any cache ledger written by v0.13.9 or earlier reproduces #1163 verbatim on the FIXED build, and adds a second false claim. `TurnSample::uncached_equivalent_usd` changed meaning from `f64` (0.0 = 'nothing could price it', v0.13.9 cache_ledger.rs:153, engine.rs:17129 `uncached_equivalent_usd: uncached.usd`) to `Option<f64>` (Some(0.0) = 'a genuine priced zero'), but `LEDGER_SCHEMA` was left at 1 — even though the module's own doc at cache_ledger.rs:72 says it is 'Bumped when a field's meaning changes'. `#[serde(default)]` therefore decodes every legacy `0.0` as `Some(0.0)`. Measured on hetzner with the freshly built binary sweep-targets/1163/debug/wayland-core against a hand-written legacy-format ledger (flux-router/flux-reasoning, cost_source=provider_reported, uncached_equivalent_usd: 0.0 — the exact shape v0.13.9 wrote, confirmed against `git show v0.13.9`): cache report → `F23_CACHE=cost usd=0.061389 uncached_equivalent_usd=0.000000 saving_usd=-0.061389 saving_ratio=unknown cost_truth=priced saving_truth=priced counterfactual_unpriced_round_trips=0` and NO saving_warning line. cache list → `F23_CACHE=total ... uncached_equivalent_usd=0.000000 cost_truth=priced`. cache verify → `trustworthy=true cost_truth=priced saving_truth=priced`, exit 0. That first line is character-for-character the report in the ticket body. The new `saving_truth=priced` is a regression in kind: the fix's own verdict now certifies the fabricated saving as trustworthy, which the pre-fix build never claimed."
  - id: c2
    text: "cache report over a v0.13.9-format ledger no longer prints saving_truth=priced with a negative saving; the run output is quoted"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D23). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "cache list's store total does not sum a legacy 0.0 into the counterfactual, and cache verify does not return trustworthy=true for it"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D23). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "A test reads a fixture ledger in the v1 on-disk shape and asserts the verdict; shown RED against today's #[serde(default)] decode"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D23). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

Reading back any cache ledger written by v0.13.9 or earlier reproduces #1163 verbatim on the FIXED build, and adds a second false claim. `TurnSample::uncached_equivalent_usd` changed meaning from `f64` (0.0 = 'nothing could price it', v0.13.9 cache_ledger.rs:153, engine.rs:17129 `uncached_equivalent_usd: uncached.usd`) to `Option<f64>` (Some(0.0) = 'a genuine priced zero'), but `LEDGER_SCHEMA` was left at 1 — even though the module's own doc at cache_ledger.rs:72 says it is 'Bumped when a field's meaning changes'. `#[serde(default)]` therefore decodes every legacy `0.0` as `Some(0.0)`. Measured on hetzner with the freshly built binary sweep-targets/1163/debug/wayland-core against a hand-written legacy-format ledger (flux-router/flux-reasoning, cost_source=provider_reported, uncached_equivalent_usd: 0.0 — the exact shape v0.13.9 wrote, confirmed against `git show v0.13.9`): cache report → `F23_CACHE=cost usd=0.061389 uncached_equivalent_usd=0.000000 saving_usd=-0.061389 saving_ratio=unknown cost_truth=priced saving_truth=priced counterfactual_unpriced_round_trips=0` and NO saving_warning line. cache list → `F23_CACHE=total ... uncached_equivalent_usd=0.000000 cost_truth=priced`. cache verify → `trustworthy=true cost_truth=priced saving_truth=priced`, exit 0. That first line is character-for-character the report in the ticket body. The new `saving_truth=priced` is a regression in kind: the fix's own verdict now certifies the fabricated saving as trustworthy, which the pre-fix build never claimed.

**Where.** crates/wcore-agent/src/cache_ledger.rs:72 (LEDGER_SCHEMA = 1, not bumped) and :173-174 (`#[serde(default, skip_serializing_if)] pub uncached_equivalent_usd: Option<f64>`); rendered at crates/wcore-cli/src/cache_cmd.rs:619-645. Doc comment at cache_ledger.rs:170-172 acknowledges the decode ('a 0.0 read back from a ledger an older build wrote decodes as Some(0.0), which is what that build meant by it') — but that is not what the old build meant by it: v0.13.9 wrote 0.0 precisely when nothing could price the model.

**Why it matters.** The cache ledger is a durable on-disk artifact whose stated purpose (cache_ledger.rs:22-24) is that a separate process can read it back without an engine. #1163 was itself filed off a persisted 0.13.9 session. So the operator who reported this bug, on upgrading, runs `cache report` over the ledger directory they already have and sees the identical negative saving — now graded `saving_truth=priced`. One legacy session also poisons `cache list`'s store total, since StoreTotals sums Some(0.0) happily. Fix is bounded: bump LEDGER_SCHEMA to 2 and have the v1 reader map `uncached_equivalent_usd: 0.0` to None (or refuse v1 rows for the saving), which is what the module's own schema rule already prescribes.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
