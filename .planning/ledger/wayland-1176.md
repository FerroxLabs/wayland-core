---
issue: 1176
repo: FerroxLabs/wayland
kind: defect
title: "Both model-limits guards are blind to provider-native passthrough ids — the #165 failure class is still uncovered"
status: open
last_verified_commit: 856df7d0
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
    evidence: "file:scripts/check-model-limits-freshness.py:1150:PASS when the vendor's output == context (UNKNOWN, not a ceiling)"
    owner: core
    note: "RE-ANCHORED 2026-08-30 for wayland#1198: was :781, a `}}` closing an unrelated bedrock fixture block. It now cites the negative control that asserts the treatment, which is what reds if `output == context` is ever read as a ceiling again. Enforcement at scan_passthrough:288 treats output == context as models.dev saying UNKNOWN; the control at :781-787 requires the grok-4.6 degenerate row to PASS rather than be graded. RE-ANCHORED 2026-08-30 by lane w3-cache-spend, position only: the SAME control line moved 988 -> 1046 when wayland#1232 inserted provider_scoped_arms and three self-test cases above it. Content unchanged and unique in the file; no re-grade, and the claim was not re-opened."
  - id: c5
    text: "The third preserved rule holds: no static arm is added for an open-weights family served at wildly different limits by different hosts"
    state: superseded
    evidence: "symbol:scripts/check-model-limits-freshness.py::scan_open_weights_arms"
    owner: core
    handoff: "FerroxLabs/wayland#1232"
    note: "RE-GRADED 2026-08-30 to not-met, and DECOMPOSED. The 2026-08-29 entry graded a SUBSTITUTED property and the verifier caught it: the open-weights branch it cited sits inside `if mid not in table:` in scan_passthrough, so it graded what the release gate DEMANDS and said nothing about what the table already HOLDS. An open-weights id that already had a row was never spread-checked at all, so nothing in the gate stopped such an arm from being ADDED -- only from being asked for. The criterion says no static arm IS ADDED for an open-weights family served at wildly different limits, and the tree contradicts that sentence, so this reads not-met. FORWARD DIRECTION CLOSED HERE: scan_open_weights_arms reads PASSTHROUGH_VENDOR_MODELS on every run and measures host spread over EVERY provider, so a new host-variable open-weights arm now FAILS the release the day it lands. RED-ARMED at 856df7d0 by deleting one line from OPEN_WEIGHTS_ARM_DEBT (mutation shown landing on the dict literal, not a comment): EXIT 1, `minimax-m2.5 ... 65536 to 228700 (3.5x) across 44 endpoints ... not listed ... a NEW violation`, against EXIT 0 on the untouched tree in the same script. Seven self-test cases prove both directions, including that a debt line for another id does not excuse this one and that a vendor-only id is out of scope. THE REMAINDER IS #1232, and it is smaller than the previous note claimed. MEASURED on the 2026-08-30 pull, twelve open-weights rows exist and SEVEN violate rule 3 -- deepseek-v4-flash 8.0x/61 hosts, deepseek-v4-flash-0731 5.1x/35, deepseek-v4-pro 8.2x/64, minimax-m2 5.1x/19, minimax-m2.1 5.1x/24, minimax-m2.5 3.5x/44, minimax-m3 4.0x/43 -- while the other five (deepseek-v4-flash-vision-exp, deepseek-v4-pro-0813, minimax-m2.5-highspeed, minimax-m2.7, minimax-m2.7-highspeed) have hosts that agree and are NOT violations; the control claude-opus-5 is 1.0x across 31 hosts. The seven are listed, dated 2026-11-30 and owned by #1232 in OPEN_WEIGHTS_ARM_DEBT, keyed on the exact model id so listing one cannot excuse the next. Removing them is blocked on product judgement, not on effort: an arm revokes should_omit_max_tokens, and #1176 c1's own test every_passthrough_vendor_model_resolves_its_arm asserts each one, so the removal and that test move together. PROVENANCE CORRECTED: the previous note said the twelve arms predate #1176 and cited e17a33b2/9d3f33c3. That conflated the ROWS with the ARMS and the hashes were wrong. Measured -- `git log --oneline -- crates/wcore-config/src/limits/passthrough.rs` returns exactly two commits and both are #1176s, so the ROWS are #1176s; `git log --oneline -S 'minimax-m2.5' -- crates/wcore-config/src/limits.rs` returns e17a33b22 alone and `-S 'deepseek-v4-pro'` returns 9d3f33c3e then 30dad572c, and `git show beb335953^:crates/wcore-config/src/limits.rs | grep -n minimax` finds minimax-m2.5 at :307 one commit before passthrough.rs existed, so the ARMS do predate it (control: `-S claude` over the same path returns 4 commits, so the query is not silently empty). #1176 added no arm; it pinned arms that were already shipping. The module doc in passthrough.rs carried the same wrong claim and is corrected. OWNERSHIP RULING, 2026-08-30 (re-grade lane), and a STATE CORRECTION. The state moves from `not-met` to `superseded` because `not-met` + `owner: core` does not decompose anything -- scripts/check-release-readiness.py counts it OUTSTANDING regardless of the `handoff:` beside it, so the handoff this note announced was decorative and #1176 went on blocking for work it had already handed away. `superseded` is the state the schema defines for exactly this -- 'a residual that was deliberately handed to another issue' -- and its precondition is met: the note names #1232, #1232 exists, and #1232 is OPEN. The `handoff:` line is kept as well because it is true and machine-readable. THE SINGLE OWNER IS FerroxLabs/wayland#1232. Three tickets contended for one piece of work -- this c5, wayland#1214 c1, and #1232 -- and #1214 c1 has been superseded onto #1232 in the same pass, so exactly one gradeable owner remains and #1232 now has its own ledger file (it had none, and the coverage check was flagging it). THE FORWARD HALF THIS NOTE CLAIMS TO HAVE CLOSED WAS RE-VERIFIED INDEPENDENTLY rather than taken on trust, because that is the claim the state change rests on: the live gate was run on 2026-08-30 (exit 0, PASS) and then red-armed by deleting the `minimax-m2.5` row from OPEN_WEIGHTS_ARM_DEBT at :233 -- a dict literal, shown before and after -- which took it to exit 1 with `FAIL -- 1 model(s) we CLAIM to cover are missing or over-claimed` naming `minimax-m2.5 ... not listed in OPEN_WEIGHTS_ARM_DEBT, so it is a NEW violation`. Restored, touched, exit 0 again. So the forward direction does fail the release the day a new host-variable open-weights arm lands, as claimed, and the only remainder is the twelve rows already in the table -- which is #1232. NOT CHANGED: the criterion's sentence is still false of the tree (seven violating arms exist and today's live run named all seven), and nothing here should be read as saying otherwise. LANE ADDENDUM (lane/f13-n-window, 2026-08-30). ROOT CAUSE, and why this is not a data edit: model_output_ceiling(_provider, model) at limits.rs:38 IGNORES the provider - the parameter is underscore-prefixed and unused - so every arm applies to every host serving that id, not only the vendor. The class fix is to scope the arms to vendor-operated providers (the PASSTHROUGH_VENDORS list already exists in the freshness script), not to delete two rows: deleting them would drop deepseek own-API users from 1,000,000 to the 32,768 unverified sentinel. NOT ATTEMPTED in this lane - a provider-scoped model_output_ceiling touches every arm and every test keyed on model_output_ceiling(provider, id), and needs its own cross-provider red arm."
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
