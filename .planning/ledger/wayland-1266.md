---
issue: 1266
repo: FerroxLabs/wayland
kind: defect
title: "ErrorInfo.category is unknown on the in-band emit_error seam and across the sub-agent relay, where the raising code knew better"
status: closed
last_verified_commit: 56b54a06e
criteria:
  - id: c1
    text: "— `OutputSink`'s error seam carries a category at every call site that has one"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1266_in_band_category_test.rs::an_engine_classified_in_band_error_reaches_the_sink_as_context_limit"
    owner: core
    note: "MET, AFTER THIS LANE CLOSED A RESIDUAL THE PREVIOUS GRADE MISSED. The seam is right: `OutputSink::emit_error` takes `category: FailureCategory` as a REQUIRED argument and `emit_run_failure` is DELETED, so a wrapper that does not forward it does not compile and there is no trait default to fall into. But c1 says 'at every call site that has one', and `ProtocolSink::emit_correlated_error` -- which builds the SAME `ProtocolEvent::Error` frame, one seam over -- still hardcoded `Unknown` and pointed readers at `emit_run_failure` for the typed answer. That method no longer exists, so the pointer went nowhere and every frame from that seam reached the host as `unknown`. FIXED: it now takes `category` from the caller, and its one caller (our own refusal of a host composer attachment) passes `LocalWayland`, which is #388's 'local Wayland error' by the same reading c2 applies to every other refused host command. GUARD, and its limit: the parameter is compiler-required, so omission cannot compile; a caller could still write `Unknown` deliberately, and nothing lints that -- recorded rather than implied away. EVIDENCE: `cargo nextest run -p wcore-agent --test issue_1266_in_band_category_test` -> `5 tests run: 5 passed`, including `an_engine_classified_in_band_error_reaches_the_sink_as_context_limit`, which drives a REAL `AgentEngine::run` to the in-band unworkable-window refusal."
  - id: c2
    text: "— the engine sites that DO know their category say so, and the ones that do not"
    state: met
    evidence: "test:crates/wcore-cli/src/main.rs::every_cli_site_holding_an_agent_error_reports_its_category"
    owner: core
    note: "MET, AFTER FOUR MORE SITES WERE FOUND AND FIXED. The classification inside the engine is right and is re-verified below. What the previous grade got wrong is the boundary it drew: it said the CLI sites 'hold only someone else's prose' and so honestly say Unknown. FOUR OF THEM DID NOT -- main.rs's headless terminal exit, the goal-driver exit, the interactive REPL exit and the host-protocol run loop each rendered `format!('{e:#}')` OUT OF the run's own `AgentError` and then passed `FailureCategory::Unknown` beside it. The most common way for a run to die therefore reached the host as `unknown` while `AgentError::failure_category()` already knew. All four now call it. NEW GUARD, graded PER CALL not per function: `every_cli_site_holding_an_agent_error_reports_its_category` walks `wcore_cli_production_sources()`, finds every `emit_error(&format!('{...` (whitespace squeezed), and requires `failure_category()` inside that call. Per-function would have passed `run`, which holds two of the four, and gone on passing with a fifth written bare. Pinned control set with counts: main.rs::repl_loop:1, main.rs::run:2, main.rs::run_json_stream_mode:1. RED ARMS RUN HERE. (a) Engine side: the tool-failure-breaker site -> `Unknown`, `touch`, `cargo check -p wcore-agent --tests` RC=0, then `the_tool_failure_breaker_reaches_the_sink_as_tool_runtime` FAILED quoting the breaker firing at 10 consecutive failures, while the controls `an_opaque_provider_failure_stays_unknown_in_band` and `the_in_band_seam_emits_more_than_one_category` stayed green. (b) New lint: one of the four CLI sites -> `Unknown`, `touch`, `cargo check -p wcore-cli --tests` RC=0, then the lint FAILED naming `fn run ... site 2`. Both restored and green. THE SITES THAT DO NOT KNOW STILL SAY UNKNOWN, deliberately: the all-attempts-failed provider exit, the empty-turn guard, autocompact wrapping an error whose origin is not visible."
  - id: c3
    text: "— a sub-agent's own failure category survives the relay in"
    state: met
    owner: core
    evidence: "test:crates/wcore-agent/tests/issue_1266_c3_subagent_relay_test.rs::a_child_that_died_on_a_context_ceiling_reaches_the_parent_as_context_limit"
    note: "MET, AND THE TEST THE PREVIOUS GRADE RECORDED AS OWED IS NOW PAID. `channel_sink.rs` passes the CHILD's category through instead of hardcoding `ToolRuntime` -- pass-through rather than a remap because c3 asks for both halves: a child that hit a context limit must arrive as `context_limit` AND a child that died on an opaque upstream response must still arrive as `unknown` rather than be upgraded to a plausible `tool_runtime`, which is exactly the guess #1237 c4 forbids made on the child's behalf. The parent still knows it was a child: `code` stays `sub_agent_error` inside the `sub_agent_event` envelope. EVIDENCE: `crates/wcore-agent/tests/issue_1266_c3_subagent_relay_test.rs` -> 3 tests, all green, covering both halves plus `two_differently_failing_children_do_not_relay_the_same_category`, which no single-constant relay can pass. RED ARM RUN HERE: restored the hardcode (`category: { let _ = category; FailureCategory::ToolRuntime }`), `touch`, `cargo check -p wcore-agent --tests` RC=0, then ALL THREE relay tests FAILED. Restored, green."
  - id: c4
    text: "— the contract cost is paid once. If wayland-core#314 c5 (an untyped `Info`"
    state: met
    evidence: "file:crates/wcore-protocol/contracts/desktop/v1/manifest.json"
    owner: core
    note: "MET, and it was met by being forced. integ/f13 had already spent 22 -> 23 on wayland-core#314 c5's `grant_refused`; the peer lane had independently spent 22 -> 23 on #1237's `ErrorInfo.category`. The merge collided on exactly that constant. Resolved as ONE 22 -> 23 changelog entry naming BOTH, and `contract.minor` moves by exactly one across the pair. RE-CONFIRMED IN THIS TREE after this lane's own SOURCE_INPUTS edits: the corpus was REGENERATED (never resolved with --theirs), `wcore-contract diff` reports `No manifest.json key moved`, schema_digest is unchanged and contract.minor is still 23 -- the c1/c2 fixes carried no second contract cost."
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
