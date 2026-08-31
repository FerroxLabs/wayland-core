---
issue: 1266
repo: FerroxLabs/wayland
kind: defect
title: "ErrorInfo.category is unknown on the in-band emit_error seam and across the sub-agent relay, where the raising code knew better"
status: open
last_verified_commit: 4251d68be
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
    state: not-met
    owner: core
    note: "REGRADED met -> not-met on 4251d68be. c3 asks that a sub-agent's own failure category REACHES THE PARENT'S HOST. It reaches the diagnostic frame only. `ChannelSink::relay_terminal` (channel_sink.rs:207) -- which its OWN doc calls `the single authoritative terminal ... a terminal that can reorder behind diagnostics is not authoritative evidence` -- hardcodes `FailureCategory::Unknown` on its Failed arm at channel_sink.rs:222, and it cannot do otherwise: its inputs are a WorkflowChildTerminalState and a &str, and spawner.rs:2530-2531 stringifies the child's AgentError (`format!(\"Sub-agent error: {error}\")`) without ever calling failure_category(). So a child dying on ContextTooLong reaches the parent's host as `unknown` on the frame the design designates authoritative. THE TEST THAT GRADES c3 CANNOT SEE THIS: issue_1266_c3_subagent_relay_test.rs states in its own header that the assertion is made on `emit_error`, and its recorded red arm mutates `impl OutputSink for ChannelSink :: emit_error` -- so deleting the category from the authoritative frame changes nothing it observes. Checked for a second grader and found none: spawn_relay_test.rs mentions relay_terminal but asserts terminal `info` events (the Succeeded arm) and never asserts category. The test is well built -- it has a real control and a two-children constant-check -- it simply grades the wrong frame for this sentence. TO CLOSE c3, SubAgentResult (or relay_terminal's signature) must carry a category first; a test written before that plumbing goes GREEN against the diagnostic frame while the authoritative frame still says unknown. --- retained, verbatim --- MET. `channel_sink.rs` passes the CHILD's category through instead of hardcoding `ToolRuntime`. Pass-through rather than a remap because c3 asks for both halves and a remap can only serve one: a child that hit a context limit must arrive as `context_limit` AND a child that died on an opaque upstream response must still arrive as `unknown` rather than be upgraded to a plausible-looking `tool_runtime` -- which would be exactly the guess #1237 c4 forbids, made on the child's behalf. The parent still knows it was a child: `code` stays `sub_agent_error` inside the `sub_agent_event` envelope. LIMIT, recorded rather than implied away: the two arms are graded by the type (the relay has no value of its own to substitute) and by the whole-workspace suite, NOT by a dedicated parent/child integration test spawning a real sub-agent. That test is owed."
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
