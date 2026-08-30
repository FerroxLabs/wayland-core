---
issue: 1237
repo: FerroxLabs/wayland
kind: defect
title: "Typed failure category on ErrorInfo: core can name three of #388's five and reports all of them as prose"
status: closed
last_verified_commit: aec2cf8c
criteria:
  - id: c1
    text: "ErrorInfo carries a typed failure category covering the three #388 names core can decide: context/token limit, tool/runtime failure, local Wayland error"
    state: met
    evidence: "symbol:crates/wcore-protocol/src/events.rs::FailureCategory"
    owner: core
    note: "ErrorInfo gains `category: FailureCategory` -- an enum, not a string -- with exactly the three decidable #388 names (ContextLimit, ToolRuntime, LocalWayland) plus Unknown. All three are produced: ContextLimit by AgentError::ContextTooLong, LocalWayland by SessionAuthority/UserAborted and by the local refusal paths (reader.rs's refused host line, startup_error.rs, main.rs's init_failed), ToolRuntime by the acp_engine TerminalGuard's engine_panic and by channel_sink's sub_agent_error. Round-tripped through the real frame per variant by every_failure_category_round_trips_through_the_error_frame, which also pins the wire alphabet to exactly the four snake_case names. ErrorInfo derives no Default and the field has none, so EVERY ErrorInfo construction in the workspace names a category or fails to compile -- `cargo check --workspace --all-targets` exit 0 is the enumeration, and the compiler rather than review is what performs it. The first check after the field landed named the four sites a scripted pass had not reached, which is the mechanism working."
  - id: c2
    text: "Every terminal error exit of the run loop sets the category, and the set of exits is enumerated rather than sampled"
    state: met
    evidence: "symbol:crates/wcore-agent/src/engine.rs::failure_category"
    owner: core
    note: "The terminal error exits of AgentEngine::run ARE the variants of AgentError -- run returns Result<AgentResult, AgentError>, so there is no terminal error exit that is not one of them -- which is why the enumeration is a match over that enum and not a list of call sites. AgentError::failure_category is exhaustive with NO wildcard arm and carries #[deny(clippy::wildcard_enum_match_arm)], so the two ways to swallow a new exit are both closed: a new variant is a compile error (non-exhaustive match) and a `_ =>` arm is a clippy hard error under the workspace -D warnings gate. every_terminal_run_exit_names_its_category drives all five exits; the_exit_classifier_has_no_default_arm_and_names_every_variant scrapes the variant list out of the enum's own declaration and asserts each appears in the classifier body and that no `_ =>` does, so a variant added tomorrow is checked without anyone updating the test. Callers: acp_engine::error_info_for, engine_bridge::emit_recovery_error, and the four main.rs run()-Err arms now pass error.failure_category() through OutputSink::emit_run_failure."
  - id: c3
    text: "The field is additive on the wire: a host that does not know it still parses the frame"
    state: met
    evidence: "test:crates/wcore-protocol/tests/issue_1237_typed_failure_category.rs::a_pre_change_error_payload_still_decodes_and_the_old_keys_do_not_move"
    owner: core
    note: "Both directions. A pre-change payload -- {code, message, retryable}, no category -- decodes, as Unknown, via serde(default) (control in the same test: a payload missing a REQUIRED key is still refused, so the decoder is not accepting anything). And the keys a pinned host already reads are unchanged: the error object is exactly {category, code, message, retryable}, nothing renamed or removed. Contract corpus regenerated: CONTRACT_MINOR 22 -> 23 (major holds at 1), forced by the wire-shape gate, which refused the regeneration under a standing 1.22 with altered=[\"events/error.json\"] -- that gate deciding the version question it exists to force. Manifest key-diff: SIX keys move (contract.minor, fixture_digest, generator, schema_digest, source_inputs_digest, and wire_shapes at exactly ONE entry, events/error.json); capabilities, child_types, commands, counts, deferred_adversarial, events, fixture_inventory, source_inputs and subcontracts are byte-identical. The schema_digest move is the deliverable here, not drift: this criterion is a contract addition, and the issue says so."
  - id: c4
    text: "A failure core cannot classify reports as unknown rather than picking one"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1237_failure_category_test.rs::a_bare_non_2xx_from_an_openai_shaped_endpoint_is_not_classified"
    owner: core
    note: "Answered as a property of the TYPE and not only as a test outcome: FailureCategory has no variant for rate_limit or router_failure, so there is no value core could emit that names the #1184 split however wrong its classification became (the_category_alphabet_cannot_name_the_router_versus_provider_split asserts the alphabet, with a known-positive control that the assertion can fail). openai.rs maps any unrecognised non-2xx to ProviderError::Api{status,message}; that shape at 500/502/503/529 comes back Unknown, and so does ProviderError::RateLimited (a 429) -- the case a classifier is most tempted to guess on, because the provider layer HAS a typed variant for it, but which side of the router rate-limited is still not decidable here. Known-positive control in the same test: ContextTooLong IS decided, so the unknowns are not everything returning one constant."
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

