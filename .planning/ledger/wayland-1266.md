---
issue: 1266
repo: FerroxLabs/wayland
kind: defect
title: "ErrorInfo.category is unknown on the in-band emit_error seam and across the sub-agent relay, where the raising code knew better"
status: open
last_verified_commit: 33167ed1fb64795d4fdbc8151a14153fb021098d
criteria:
  - id: c1
    text: "— `OutputSink`'s error seam carries a category at every call site that has one"
    state: met
    evidence: "test:crates/wcore-cli/tests/issue_1266_c1_host_frame_e2e.rs::the_context_ceiling_refusal_reaches_the_real_host_as_context_limit"
    owner: core
    note: "MET. `OutputSink::emit_error` now takes `category: FailureCategory` as a REQUIRED argument and `emit_run_failure` is DELETED. Deleting it is the point rather than a tidy-up: once emit_error carries a category the two have the same signature and body at every override, and the only thing the second still contributed was its trait DEFAULT, which this issue's own comment measured flattening a category for a delegating wrapper. One method means a wrapper that does not forward it does not compile and there is no default to fall into -- omission is a compile error, which is what c1 asks for. Enumerated by the compiler: 64 `fn emit_error` definitions and 85 call sites. EVIDENCE: `cargo nextest run -p wcore-agent --test issue_1266_in_band_category_test` -> `5 tests run: 5 passed`, including `an_engine_classified_in_band_error_reaches_the_sink_as_context_limit`, which drives a REAL `AgentEngine::run` to the in-band unworkable-window refusal. RED ARM: that call site -> Unknown, `touch`, `cargo check -p wcore-agent --tests` RC=0, then `5 tests run: 3 passed, 2 failed`."
  - id: c2
    text: "— the engine sites that DO know their category say so, and the ones that do not"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1266_c2_frame_test.rs::a_budget_refusal_reaches_the_host_frame_as_local_wayland"
    owner: core
    note: "MET. Classified against FailureCategory's own doc text, not against what reads plausibly: ContextLimit on the context-ceiling and output-ceiling exits, ToolRuntime on the tool-failure breaker / no-progress loop / two mid-flight strategy breakers / an MCP backend that would not come up, LocalWayland on session-persistence faults, every budget and spend-guard refusal, our own mid-flight monitor, and on the CLI side every refused or malformed host command plus the startup failure. Five sites hold a real `AgentError` and call `failure_category()` rather than re-deciding. THE ONES THAT DO NOT KNOW STILL SAY UNKNOWN, deliberately: the all-attempts-failed provider exit, the empty-turn guard, autocompact wrapping an error whose origin is not visible, and the CLI sites holding only someone else's prose. EVIDENCE: `the_tool_failure_breaker_reaches_the_sink_as_tool_runtime` (real run, real breaker -- the red-arm failure message quotes it firing at 10 consecutive failures) plus the control `an_opaque_provider_failure_stays_unknown_in_band`, and `the_in_band_seam_emits_more_than_one_category`, which no single-constant classifier can pass. RED ARM: the ToolRuntime site -> Unknown, `touch`, rebuilt, `5 tests run: 4 passed, 1 failed`."
  - id: c3
    text: "— a sub-agent's own failure category survives the relay in"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1266_c3_authoritative_terminal_test.rs::a_context_ceiling_child_reaches_the_authoritative_terminal_as_context_limit"
    owner: core
    note: "MET on the AUTHORITATIVE frame, which is the one the previous grading could not see. The 2026-08-31 re-grade was right: `ChannelSink::relay_terminal` hardcoded `FailureCategory::Unknown` on its `Failed` arm and could not do otherwise -- its inputs were a `WorkflowChildTerminalState` and a `&str`, and the spawner stringified the child`s `AgentError` without ever calling `failure_category()`. THE PLUMBING NAMED THERE IS NOW BUILT: `SubAgentResult` carries `failure_category`, `relay_terminal` takes it as a REQUIRED argument, and the whole chain is compiler-enumerated -- 20 construction sites, each naming a category it can defend. That required `FailureCategory` to sit below `wcore-protocol` in the crate graph, so it moved to `wcore-types` (its semantic home: a provider-neutral failure taxonomy) with a re-export from `wcore_protocol::events`, and the new path was added to contract `SOURCE_INPUTS` so the corpus still hashes a wire-visible type`s definition. `wcore-contract check` reports `schema_digest` UNCHANGED (sha256:8497e92e...) -- a source-hash rebase, NOT a wire change, so no `CONTRACT_MINOR` is owed and `minor` stays 23; corpus regenerated, `cargo nextest run -p wcore-protocol` 436/436. BOTH HALVES ASSERTED, and they turn out to travel by DIFFERENT production sites -- established by probe, not assumed: the context-ceiling child`s run returns `Ok` with `finish_reason = Length` and is classified in `subagent_ok_result`; the opaque child`s run returns `Err(ApiError)` and is classified by `error.failure_category()` in `execute_resolved_launch`. TWO RED ARMS, BOTH MEASURED, and THE FIRST ONE WRITTEN WAS VACUOUS AND IS RECORDED AS SUCH: mutating `error.failure_category()` to `Unknown` reddened NOTHING (7 passed) because `Unknown` is the opaque arm`s own expected value. ARM A (`FinishReason::Length` -> `ToolRuntime`): 3 passed, 1 FAILED, the context assertion. ARM B (`error.failure_category()` -> `ToolRuntime`): 5 passed, 2 FAILED, the opaque assertion here AND its sibling in `issue_1266_c3_subagent_relay_test.rs`. Each mutation was grepped to confirm it landed on the CODE line and `cargo check -p wcore-agent --tests` reported 0 errors first, so each red is behaviour and not a build break. WRONG-REFUSAL CONTROL: `an_opaque_upstream_child_still_reaches_the_authoritative_terminal_as_unknown` -- carrying a category through must not start MANUFACTURING one, and a plausible `tool_runtime` there is the guess #1237 c4 forbids. A LANE-SEPARATION CONTROL is included because grading the wrong frame is how this row went wrong before: `the_authoritative_terminal_is_a_separate_lane_from_the_diagnostics` asserts exactly one authoritative terminal AND a non-empty diagnostic stream, so a change that routed diagnostics into the terminal collector cannot make the file pass. Both round-trip codecs (durable child payload, fleet shard payload) carry the category additively so neither becomes a new drop point. Whole workspace green: `cargo check --workspace --all-targets` 0 errors."
  - id: c4
    text: "— the contract cost is paid once. If wayland-core#314 c5 (an untyped `Info`"
    state: met
    evidence: "symbol:crates/wcore-protocol/src/contract/generate.rs::CONTRACT_MINOR"
    owner: core
    note: "MET, and it was met by being forced. integ/f13 had already spent 22 -> 23 on wayland-core#314 c5's `grant_refused`; the peer lane had independently spent 22 -> 23 on #1237's `ErrorInfo.category`. The merge collided on exactly that constant, which is this criterion arriving as a conflict. Resolved as ONE 22 -> 23 changelog entry naming BOTH, and `contract.minor` moves by exactly one across the pair. The corpus was regenerated from the merge base at 1.22 rather than resolved with --theirs, so the published 1.23 carries `events/grant_refused.json` AND `error.category` together; `wcore-contract diff` reports no manifest key moved."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
