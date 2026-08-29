---
issue: 1176
repo: FerroxLabs/wayland
kind: defect
title: "Both model-limits guards are blind to provider-native passthrough ids — the #165 failure class is still uncovered"
status: open
last_verified_commit: 9de21aa1
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
    evidence: "file:scripts/check-model-limits-freshness.py:781"
    owner: core
    note: "Enforcement at scan_passthrough:288 treats output == context as models.dev saying UNKNOWN; the control at :781-787 requires the grok-4.6 degenerate row to PASS rather than be graded."
  - id: c5
    text: "The third preserved rule holds: no static arm is added for an open-weights family served at wildly different limits by different hosts"
    state: not-met
    evidence: "file:scripts/check-model-limits-freshness.py:156"
    owner: core
    note: "Added 2026-08-29; the acceptance text names three preserved rules and the ledger claimed only two. PASSTHROUGH_IN_SCOPE at :156 narrows qwen to qwen3.N-(max|plus|flash) and llama to llama-4-maverick/scout and llama-3.3-, and PASSTHROUGH_VENDORS lists only vendor-operated hosts, so the qwen3.6-27b shape cannot acquire an arm. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: BOTH the pointer and the property fail. (1) POINTER: `file:scripts/check-model-limits-freshness.py:156` is PASSTHROUGH_IN_SCOPE, whose keys are claude, gpt, grok, gemini, deepseek, minimax -- there is NO qwen or llama key in it. The note's description ('narrows qwen to qwen3.N-(max|plus|flash) and llama to llama-4-maverick/scout and llama-3.3-') actually describes lines 113 and 116, a DIFFERENT dict belonging to the pre-existing CATALOGUE scan, which #1176 did not author. The criterion was graded met against code from another change. (2) PROPERTY: passthrough.rs's module doc states 'No open-weights family is listed here: those live in CATALOGUE_CEILINGS, where the same rule already applies.' That claim is false on its own contents. PASSTHROUGH_VENDOR_MODELS lists minimax-m2, m2.1, m2.5, m2.7, m3 and deepseek-v4-pro/flash, and every one has a static arm in the if-chain (limits.rs:306-314, :~265). Measured against the live models.dev pull I fetched today: `minimax-m2.5` has 46 host rows with distinct contexts {65,536 | 192,000 | 196,000 | 196,608 | 196,680 | 197,000 | 200,000 | 204,000 | 204,800 | 228,700} and outputs from 8,192 (nebius) and 16,000 (cloudferro) up to 196,608 -- against a static arm of (128_000, 204_800). `deepseek-v4-pro` has 74 host rows across ten distinct contexts {128,000 ... 1,050,000} with outputs from 8,192 (frogbot) and 16,384 (deepinfra) -- against a static arm of (384_000, 1_000_000). That is textbook 'served at wildly different limits by different hosts'. The repo's own test proves the standard being applied inconsistently: `host_variable_open_weights_stay_unknown` (limits/catalogue.rs:433) forces qwen3.6-27b et al. to `None` with the stated reason 'anything at or above the 200,000 CompactConfig default makes the small hosts WORSE than the status quo' -- and minimax-m2.5's arm is 204,800, above that same DEFAULT_CONTEXT_WINDOW (compact.rs:12). (3) WORSE, the replacement MECHANIZES more of them: PASSTHROUGH_IN_SCOPE's `minimax: ^minimax-m(?:[2-9]|/d/d)` and `deepseek: ^deepseek-v(?:[4-9]|/d/d)` are documented as FLOORS ('this generation and everything above it'), so the release gate will now go red on a future minimax-m4 / deepseek-v5 with the instruction 'Add the arm if it has none, then add the row.' The third preserved rule is not preserved by the replacement; it is inverted and automated. In fairness: the minimax and deepseek arms predate #1176 (git log -S dates minimax-m2.5 to e17a33b2, the #165 fix, and deepseek-v4-pro to 9d3f33c3), so #1176 added no such arm itself -- but it now asserts those exact figures in a per-PR Rust test and grades them only against vendor-operated rows, which structurally cannot see the spread."
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
