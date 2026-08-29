---
issue: 1176
repo: FerroxLabs/wayland
kind: defect
title: "Both model-limits guards are blind to provider-native passthrough ids — the #165 failure class is still uncovered"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "A guard covers model ids that reach users by provider-native --model passthrough in the if-chain families"
    state: met
    evidence: "test:crates/wcore-config/src/limits.rs::every_passthrough_vendor_model_resolves_its_arm"
    owner: core
    note: "Walks PASSTHROUGH_VENDOR_MODELS (83 rows, crates/wcore-config/src/limits/passthrough.rs) through the real model_output_ceiling lookup, so the if-chain evaluates itself. Three sibling tests pin the premise: provider_specific_spellings_resolve_through_the_same_arm, passthrough_table_is_populated_and_free_of_duplicates, and the_passthrough_table_covers_ids_the_routed_guard_cannot_see."
  - id: c2
    text: "That guard is demonstrated going red when an arm for a passthrough id is deliberately removed"
    state: met
    evidence: "symbol:scripts/check-model-limits-freshness.py::self_test"
    owner: core
    note: "P2 (FAIL when a vendor passthrough id has no row - the #165 shape) is the acceptance red arm, with P3/P4/P5 as three further FAIL directions and P1/P6/P7/P8 as controls. Wired to CI on every PR (ci.yml:1338) and at release (release.yml:71). CAVEAT: the red is demonstrated on the SCRIPT arm by removing a row; the Rust arm's None means no-arm-at-all branch is structural and has never been exercised red."
  - id: c3
    text: "The replacement builds consensus from vendor-operated providers only, never aggregators"
    state: met
    evidence: "symbol:scripts/check-model-limits-freshness.py::scan_passthrough"
    owner: core
    note: "Grades only providers listed in PASSTHROUGH_VENDORS - the vendor's own API plus first-party resale. P8 is the negative control proving an aggregator publishing junk for an in-scope id cannot move the verdict. The one vendor-versus-vendor disagreement (Bedrock's stale 64,000 Sonnet 4.6 output) is PINNED to all four numbers rather than muted."
  - id: c4
    text: "The replacement treats output equal to context as models.dev saying unknown, never as a ceiling"
    state: met
    evidence: "file:scripts/check-model-limits-freshness.py:781:# P7. NEGATIVE CONTROL: output == context is UNKNOWN, never a ceiling."
    owner: core
    note: "Enforcement at scan_passthrough:288 treats output == context as models.dev saying UNKNOWN; the control at :781-787 requires the grok-4.6 degenerate row to PASS rather than be graded."
  - id: c5
    text: "The third preserved rule holds: no static arm is added for an open-weights family served at wildly different limits by different hosts"
    state: met
    evidence: "file:scripts/check-model-limits-freshness.py:156:PASSTHROUGH_IN_SCOPE = {"
    owner: core
    note: "Added 2026-08-29; the acceptance text names three preserved rules and the ledger claimed only two. PASSTHROUGH_IN_SCOPE at :156 narrows qwen to qwen3.N-(max|plus|flash) and llama to llama-4-maverick/scout and llama-3.3-, and PASSTHROUGH_VENDORS lists only vendor-operated hosts, so the qwen3.6-27b shape cannot acquire an arm."
---

Two automated guards protect `crates/wcore-config/src/limits.rs`:
`scripts/check-model-limits-freshness.py` at release time, and the #165 drift
test `every_routed_catalog_model_has_a_known_window`. They share a blind spot,
and it is exactly where #165 came from. The freshness script cannot evaluate the
older `if`-chain families and says so; the drift test walks
`models_for_provider()`, which is routed aliases only. A model id that is in an
`if`-chain family and reaches users through provider-native passthrough is
covered by neither.

A hand check against a live models.dev pull found three real defects both guards
passed over: `claude-opus-5` had no arm at all (a missing arm does not fall back
to 200,000 — it becomes the 32,768 unverified sentinel, a 30x undersize on
Anthropic's flagship, with output simultaneously clamped to 8,192),
`gpt-4o-2024-05-13` over-claimed output 4x, and `gemini-flash-latest` had no
arm. Those three arms are now present in `limits.rs`, but the issue is explicit
that it is about the guards and not those arms, so no criterion here claims them.

Nothing has been built against the guards. The hand check remains the only thing
covering this class, which is the situation that produced #165.
