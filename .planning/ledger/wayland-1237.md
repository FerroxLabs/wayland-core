---
issue: 1237
repo: FerroxLabs/wayland
kind: defect
title: "Typed failure category on ErrorInfo: core can name three of #388's five and reports all of them as prose"
status: closed
last_verified_commit: 4e4f9d53f
criteria:
  - id: c1
    text: "ErrorInfo carries a typed failure category covering the three #388 names core can decide: context/token limit, tool/runtime failure, local Wayland error"
    state: met
    evidence: "symbol:crates/wcore-protocol/src/events.rs::FailureCategory"
    owner: core
    note: "MET. `ErrorInfo` carries `category: FailureCategory` -- an enum with exactly the three decidable #388 names (ContextLimit, ToolRuntime, LocalWayland) plus Unknown. Re-verified in THIS tree, where it is load-bearing rather than asserted: sixteen `ErrorInfo` construction sites written on integ/f13 after the peer lane branched FAILED TO COMPILE on merge with `error[E0063]: missing field `category``, which is the no-Default claim doing its job on code that had never seen it."
  - id: c2
    text: "Every terminal error exit of the run loop sets the category, and the set of exits is enumerated rather than sampled"
    state: met
    evidence: "symbol:crates/wcore-agent/src/engine.rs::failure_category"
    owner: core
    note: "MET. The terminal error exits of `AgentEngine::run` ARE the variants of `AgentError` (`run` returns `Result<AgentResult, AgentError>`), so the enumeration is an exhaustive match over that enum with no wildcard arm and `#[deny(clippy::wildcard_enum_match_arm)]`. `cargo clippy --workspace --all-targets -- -D warnings` is clean in this tree."
  - id: c3
    text: "The field is additive on the wire: a host that does not know it still parses the frame"
    state: met
    evidence: "test:crates/wcore-protocol/tests/issue_1237_typed_failure_category.rs::a_pre_change_error_payload_still_decodes_and_the_old_keys_do_not_move"
    owner: core
    note: "MET, and the additivity was re-proved by the merge itself: the corpus was regenerated from the MERGE BASE (70a47aaed, contract 1.22) rather than resolved from either side, and `wcore-contract diff` then reported `No manifest.json key moved ... schema_digest sha256:8497e92e4ab2599201f95b2aa62c359ae2328429305e79a96761356483fc6e33`. A pre-change payload with no `category` still decodes as Unknown via serde(default), with a control in the same test that a payload missing a REQUIRED key is still refused."
  - id: c4
    text: "A failure core cannot classify reports as unknown rather than picking one"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1237_failure_category_test.rs::a_bare_non_2xx_from_an_openai_shaped_endpoint_is_not_classified"
    owner: core
    note: "MET as a property of the TYPE: `FailureCategory` has no variant for rate_limit or router_failure, so there is no value core could emit that names the #1184 split however wrong a classifier became. Re-confirmed end to end in this tree by #1266's `an_opaque_provider_failure_stays_unknown_in_band`, which drives a real run to the all-attempts-failed exit and asserts `Unknown` over EVERY error the run emitted."
---

Decomposed from FerroxLabs/wayland#388 c7 on 2026-08-30, by the lane that
closed #388's harness half.

#388 asks the product to expose which of five things went wrong. Two of them —
provider rate limit and router failure — are indistinguishable from outside the
router and belong to #1184. The other three are decidable inside core today and
still reach the host as prose, so a Desktop app, a JSON-stream consumer or a CI
wrapper has to pattern-match English to find out why a long run died.

Split out rather than closed inside #388 for two reasons. It is a contract
addition to a pinned protocol surface, which is a different kind of change from
the harness fixes that lane shipped. And it overlaps wayland-core#314 c5 — the
same shape, an untyped frame where a typed one is owed, on the grant-refusal
path — so doing them separately means two contract bumps for one contract
change. Whoever takes either should read both.

## What landed, and what did not

Delivered as a contract addition, which is what the issue asked for: an enum on
the wire, a minor bump the wire-shape gate forced, and a corpus regeneration.

Two things are deliberately NOT in scope and are named rather than left for a
reader to discover.

`OutputSink::emit_error` -- the IN-BAND seam the engine calls ~30 times -- was
not widened. It receives prose and a retryable flag, and deciding a category
from prose is the defect this ticket reports; widening it would spread
`Unknown`, not information. The typed answer travels on the new
`emit_run_failure`, whose default is safe because a sink that serialises a
host-facing frame has to build an `ErrorInfo`, and `ErrorInfo` has no
`Default`. The residual: an in-band engine error that the engine itself knows
the category of still reaches the host as `unknown`.

`channel_sink`'s sub-agent relay reports `tool_runtime` for a failed child --
correct from the PARENT turn's point of view, since the child is something the
parent invoked -- but the child's own category is not carried across the relay
boundary.

Both are the same shape as wayland-core#314 c5, which this ticket's notes
already point at: an untyped frame where a typed one is owed. If #314 c5 lands
in this release, it should ride the SAME contract bump rather than a second one.

