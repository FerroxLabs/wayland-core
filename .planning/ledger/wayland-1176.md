---
issue: 1176
repo: FerroxLabs/wayland
title: "Both model-limits guards are blind to provider-native passthrough ids — the #165 failure class is still uncovered"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "A guard covers model ids that reach users by provider-native --model passthrough in the if-chain families"
    state: not-met
    owner: core
    note: "the freshness script states this gap in its own output, and the drift test walks routed aliases only"
  - id: c2
    text: "That guard is demonstrated going red when an arm for a passthrough id is deliberately removed"
    state: not-met
    owner: core
    note: "a guard that cannot be shown to fail is producing a green that reads like coverage, which is how #165 happened"
  - id: c3
    text: "The replacement builds consensus from vendor-operated providers only, never aggregators"
    state: not-met
    owner: core
    note: "aggregator rows publish ctx=0, out=1010000 and dropped digits; this rule was load-bearing this cycle"
  - id: c4
    text: "The replacement treats output equal to context as models.dev saying unknown, never as a ceiling"
    state: not-met
    owner: core
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
