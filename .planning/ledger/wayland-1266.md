---
issue: 1266
repo: FerroxLabs/wayland
kind: defect
title: "ErrorInfo.category is unknown on the in-band emit_error seam and across the sub-agent relay, where the raising code knew better"
status: open
last_verified_commit: b47e9b94
criteria:
  - id: c1
    text: "— `OutputSink`'s error seam carries a category at every call site that has one to give, and the set of call sites is enumerated by the compiler rather than by a reviewer. Evidence: the signature change (or an equivalent that makes omission a compile error, not a default), plus a test that an engine-classified in-band error arrives at the host with a category other than `unknown`."
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1266_in_band_category_test.rs::an_engine_classified_in_band_error_reaches_the_sink_as_context_limit"
    owner: core
    note: "MET. Both halves of the evidence clause are in the tree. (a) The signature: `OutputSink::emit_error` takes `category: FailureCategory` as a REQUIRED argument and the second method `emit_run_failure` is DELETED, so a sink that omits a category does not compile and there is no trait default left to fall into -- which is the \"equivalent that makes omission a compile error, not a default\" the criterion offers as an alternative to widening. `cargo check -p wcore-protocol -p wcore-agent -p wcore-cli --tests` exit 0 IS the enumeration, performed by the compiler; the lane counted 64 `fn emit_error` definitions and 85 call sites. (b) The test: `an_engine_classified_in_band_error_reaches_the_sink_as_context_limit` drives a real `AgentEngine::run` over a scripted provider whose `[compact] context_window` trips the in-band `unworkable_window_refusal`, and asserts the frame arrives as `context_limit` -- not `unknown`. It records a re-runnable red arm (revert that site to `Unknown`, `touch`, rebuild). Graded on 2026-08-31 against b47e9b94. ADDITIONALLY closed here: `ProtocolSink::emit_correlated_error` (the pre-engine sibling of the trait seam, one call site in main.rs) hardcoded `Unknown` and justified it by deferring the typed answer to `emit_run_failure` -- a method that no longer exists. It now takes `category` as a required argument and the call site (a rejected composer attachment) names `LocalWayland`, so the last inherent hardcode on this seam is gone too."
  - id: c2
    text: "— the engine sites that DO know their category say so, and the ones that do not still say `unknown`. Evidence: a test per category asserting an in-band frame, and a control that a genuinely unclassifiable in-band error is still `unknown` rather than being given a plausible-looking value."
    state: not-met
    owner: core
    note: "NOT MET -- one arm of the evidence clause is absent. The criterion asks for \"a test PER CATEGORY asserting an in-band frame\" plus the unclassifiable control. Present in `crates/wcore-agent/tests/issue_1266_in_band_category_test.rs`: `context_limit` (`an_engine_classified_in_band_error_reaches_the_sink_as_context_limit`), `tool_runtime` (`the_tool_failure_breaker_reaches_the_sink_as_tool_runtime`), and the control (`an_opaque_provider_failure_stays_unknown_in_band`, which also carries a known-positive sibling asserting the alphabet is not one constant). ABSENT: `local_wayland`. `grep -n 'LocalWayland\\|local_wayland' ` over that test file returns nothing. The lane's own issue comment claims `LocalWayland` on session-persistence faults, every budget and spend-guard refusal, the mid-flight monitor, and on the CLI side every refused or malformed host command plus the startup failure -- three of the four categories the seam can emit are therefore asserted through the production path and the largest group of sites is asserted by review. That is the shape this issue exists to remove, so it is graded not-met rather than met-with-a-note. A single test driving one LocalWayland site (the budget refusal is the cheapest to arm) closes it."
  - id: c3
    text: "— a sub-agent's own failure category survives the relay in `channel_sink.rs`. Evidence: a test that a child failing on a context limit reaches the parent's host as `context_limit`, with a control that a child failing on an opaque upstream response still reaches it as `unknown`."
    state: not-met
    owner: core
    note: "NOT MET -- and the gap is in the PRODUCT, not only in the missing test. The lane recorded only that \"a dedicated integration test spawning a real sub-agent is owed\". Reading the relay end to end says more than that: c3's own first arm does not hold today for a child that dies on a TERMINAL `AgentError`. Two relay paths exist in `channel_sink.rs`. (1) `ChannelSink::emit_error` -- the child engine's IN-BAND errors -- does pass the child's category through verbatim (line ~373), which is real and is what the lane built. (2) `ChannelSink::relay_terminal`, which the same file calls \"the single authoritative terminal ... There is deliberately no stream fallback: a terminal that can reorder behind diagnostics is not authoritative evidence\", HARDCODES `FailureCategory::Unknown` on its `Failed` arm (line 222). It cannot do otherwise: its only inputs are a `WorkflowChildTerminalState` and a message, and its caller `spawner.rs::relay_subagent_terminal` is fed a `SubAgentResult`, which carries no category either. At `spawner.rs:2517` the child's `engine.run(...)` `Err(AgentError)` is stringified -- `format!(\"Sub-agent error: {error}\")` -- and `failure_category()` is never called on it. So a child failing on `AgentError::ContextTooLong` reaches the parent's host as `unknown` on the one frame the design designates as authoritative, which is the exact arm c3 names. Closing this needs a category on `SubAgentResult` (or on `relay_terminal`'s signature) before the test is worth writing; the test alone would be written against the diagnostic frame and would pass while the authoritative one still says `unknown`."
  - id: c4
    text: "— the contract cost is paid once. If wayland-core#314 c5 (an untyped `Info` frame where a typed one is owed, on the grant-refusal path) is still open when this lands, both ride ONE `CONTRACT_MINOR` bump. Evidence: the `CONTRACT_MINOR` changelog entry names both, and `contract.minor` moves by exactly one across the pair."
    state: met
    evidence: "symbol:crates/wcore-protocol/src/contract/generate.rs::CONTRACT_MINOR"
    owner: core
    note: "MET. Verified against the artifact on 2026-08-31 at b47e9b94, both halves. (a) The changelog: the 22 -> 23 block in `crates/wcore-protocol/src/contract/generate.rs` now names BOTH additions under one heading -- (a) wayland-core#314 c5's `grant_refused` event and (b) wayland#1237's optional `category` field on `error`'s `ErrorInfo`, with its feature-detection rationale (\"a host CANNOT feature-detect this by looking\"). It did NOT before: the merge that resolved the `CONTRACT_MINOR` collision (3e3cb3820) kept integ/f13's `grant_refused` entry and DROPPED the lane's `error.category` entry while its own message asserted \"Resolved as one 22 -> 23 entry naming both\", so a host pinned below 1.23 reading the changelog to learn what 23 added was told about `grant_refused` and was NOT told `error.category` exists. The dropped paragraph was recovered from `216b37cf6` and merged rather than rewritten. (b) The move: `contracts/desktop/v1/manifest.json` is `{\"major\":1,\"minor\":23}` against the merge base's 22 -- exactly one -- and the published 1.23 carries BOTH `events/grant_refused.json` and `category` in `events/error.json`. Corpus regenerated with `wcore-contract generate` after the edit (generate.rs, protocol_sink.rs and main.rs are all `SOURCE_INPUTS`), never resolved with `--theirs`; key-diffed old vs new, exactly two keys moved -- `fixture_digest` and `source_inputs_digest` -- and `schema_digest` HOLDS at sha256:8497e92e4ab2599201f95b2aa62c359ae2328429305e79a96761356483fc6e33, so no wire schema moved. `cargo test -p wcore-protocol`: all suites pass."
---

Created 2026-08-31 to close a COVERAGE gap, then REPAIRED and re-graded the same
day. The first version recorded no work as done and its four criteria texts were
TRUNCATED at the issue body's first line-wrap -- c2 stopped at "and the ones that
do not", c3 at "survives the relay in", c4 at "an untyped `Info`" -- while calling
itself "transcribed from the issue body without edit". A truncated criterion is
strictly worse than a loose one: c2 lost the clause naming its control, c3 lost
the file it is about AND both of its arms, and c4 lost the entire evidence
standard, so every one of them could have been graded met by an easier adjacent
property with the ledger's own text as cover. All four are now restored verbatim
from `gh issue view 1266 -R FerroxLabs/wayland` (body plus both comments) --
including the `Evidence:` sentence each bullet carries, which IS the standard and
is not separable from the claim.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on wayland
and EVERY open issue on wayland-core. This issue was in scope from the moment it
was filed and had no ledger file, so `scripts/check-release-readiness.py` -- which
reads ledger files and nothing else -- could not count it. CI runs the coverage
gate with `--offline`, the arm that would have reported the gap, so nothing said
so for two days.

Grading is against the tree at b47e9b94, not against the issue's own comments.
Where a comment and the artifact disagree the artifact wins: comment 1 asserted
c4 met via a merge that "Resolved as one 22 -> 23 entry naming both", and the
merge had in fact dropped one of the two entries -- that is what this branch
repairs, and only after the repair is c4 met. c3's comment recorded a missing
test; reading the relay end to end found a missing category on the authoritative
terminal frame as well, which the test would not have caught because it would
have been written against the other frame.

Criteria texts are transcribed WITHOUT edit. Where the body's wording is loose it
is LEFT loose rather than tightened here: sharpening a criterion inside the ledger
is how a criterion quietly becomes an easier adjacent property. Whoever takes the
two open criteria restates them on the ISSUE first.
