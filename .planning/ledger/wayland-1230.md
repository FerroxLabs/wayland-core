---
issue: 1230
repo: FerroxLabs/wayland
kind: defect
title: "A served context slot below core's own baseline turn still truncates: 2,457 of a 4,096 slot is tool schemas before the user types"
status: open
last_verified_commit: 94c1709b
criteria:
  - id: c1
    text: "— The un-compactable floor of a turn (system prompt + tool schemas —"
    state: met
    evidence: "test:crates/wcore-agent/src/compact/estimate.rs::floor_tracks_the_tool_schemas_it_is_given"
    owner: core
    note: "SECOND INSTRUMENT CONFIRMS MET, with a named ANCHOR DEFECT. Production call sites counted independently, excluding the definition and doc comments: TWO real ones -- crates/wcore-agent/src/engine.rs:13691 (the turn loop) and crates/wcore-agent/src/engine.rs:9093 (AgentEngine::uncompactable_turn_floor, the grading surface). The turn-loop site computes the floor from the SAME `system` and `tools` bindings that are moved unchanged into `LlmRequest` at engine.rs:13784-13788; I read the intervening ~90 lines and nothing rebinds either. INDEPENDENTLY MEASURED on this tree (temporary probe test, removed): the assembled floor is 4,636 estimator tokens while the RAW registry floor is 19,101 over 49 schemas, so the `assemble_turn_prelude` extraction the lane made the centrepiece of c1 is genuinely load-bearing and the test`s `raw >= assembled` assertion is not a tautology. ANCHOR DEFECT (my mutation N1, independent of the lane`s M3): replacing the body of `uncompactable_floor_tokens` with a plausible HARDCODED constant (4_010) -- the exact shape c1 forbids -- compiles (`cargo check -p wcore-agent --tests` RC=0) and leaves THIS CRITERION`S CITED ANCHOR GREEN. What actually reds are two tests the ledger does not cite: `compact::estimate::tests::floor_tracks_the_tool_schemas_it_is_given` and `floor_is_a_floor_for_every_message_list` (both FAIL, both retries). So the criterion holds on the product but the anchor alone does not discharge it; the anchor is repointed below to the test that does. Restored + touched, 97/97 scoped tests green."
  - id: c2
    text: "— `BASELINE_TURN_TOKENS = 3,118` (`compact.rs:126`) is a snapshot of a"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1230_context_floor_test.rs::the_baseline_constant_still_describes_this_tree"
    owner: core
    note: "SECOND INSTRUMENT CONFIRMS MET, and independently CONFIRMS the lane`s refusal to raise the constant. Measured headroom, not asserted: the guard prints `assembled floor 4636 estimator tokens = 3605 real-equivalent vs BASELINE_TURN_TOKENS 3118; drift 15.6%` against DRIFT_TOLERANCE 0.25, so it reds once the assembled floor grows about 8% (~370 estimator tokens, ~1.1k characters of schema) -- a second built-in tool family or an always-on MCP server clears that. Non-vacuous. THE REPOINT IS ARITHMETICALLY CORRECT and I re-derived it rather than accepting it: on this tree `minimum_workable_window() == 6929`, `supports_compaction(8192) == true`, `input_ceiling_for_window(8192) == 5053` and `autocompact_threshold_for_window(8192) ~= 3688`. Raising BASELINE_TURN_TOKENS to 4,636 puts it above that 3,688 autocompact boundary, so `supports_compaction(8_192)` flips to FALSE and `minimum_workable_window` moves to roughly 10,300 -- exactly the harm the lane names, and it would break this issue`s own c5. Leaving the constant alone is right. LIMIT OF THE GUARD, recorded because it is not obvious: under mutation N1 (floor hardcoded to 4_010, i.e. BASELINE x 1.286) this test reports a 0.02% drift and stays GREEN. It is a DRIFT guard, not a COMPUTEDNESS guard -- which is all c2 asks for, but it means c1 and c2 share one anchor that neither of them alone can fail on the property c1 is about."
  - id: c3
    text: "— At a served window below that floor core takes a **named** decision,"
    state: met
    evidence: "symbol:crates/wcore-agent/src/engine.rs::context_floor_refusal"
    owner: core
    note: "SECOND INSTRUMENT: the DECISION is met; the MECHANISM that reaches it is UNGRADED IN-TREE, and I proved that rather than inferring it. Met half: `AgentEngine::context_floor_refusal` has exactly ONE real production call site (engine.rs:13693), before any dispatch, and the message names the model, the slot, the computed floor, the derived input ceiling, the tool count and the remedy window. I saw it fire live on my own binary and my own endpoint, wording independently reproduced: `...serving `qwen3:8b` with a 4096-token context slot, and the floor of this turn is 4097 tokens ... Raise the context length of the server to at least 6644 tokens`. VACUITY GAP, MUTATION N2: making `wcore_providers::ollama_probe::probe_ollama_served_window` return `None` unconditionally -- severing the probe -> `compact_state.stated_window` wiring, which is the ONLY thing that lets any decision happen before the first request -- compiles (`cargo check -p wcore-agent --tests` RC=0) and leaves the FULL suite green: `cargo nextest run -p wcore-agent -p wcore-providers` = 5,099 tests run, 5,099 passed. Not one test in either crate grades that link. The three probe tests cover a fixture parse, a CLOSED PORT, and a non-Ollama provider; none covers a reachable endpoint that answers. The link is graded ONLY by the c4 live run, which CI cannot re-run. SECOND, MINOR: mutation N3, flipping the deciding boundary to `ceiling >= floor`, also compiles and is NOT caught -- `the_floor_gate_refuses_exactly_the_windows_that_cannot_hold_the_floor` derives its expected answer from the same `input_ceiling_for_window` call the gate uses, on a 64-token step, so it never lands on the `ceiling == floor` point where the two differ. Both mutations restored + touched; 97/97 scoped green afterwards."
  - id: c4
    text: "— LIVE PROOF, not a mock: a run against a real stock Ollama serving"
    state: unmet
    evidence: "symbol:crates/wcore-agent/src/engine.rs::refresh_stated_served_window"
    owner: core
    note: "REFUTED BY AN INDEPENDENT LIVE RUN. The lane`s passing arm ran against a WARM server and the ordinary case is a COLD one. Evidence, mine end to end: binary sha256 118c8466342cc5d569fab0e763785a11cab0ce58fd93613927c90f5dd5cd724e built in /root/tgt-s2/context-floor-audit; my own Ollama on port 21437 at OLLAMA_CONTEXT_LENGTH=4096 (`n_ctx_slot = 8192` equivalent line in its own log for the sister instance; ps reports context_length 4096), never the ambient service on 11434; the same byte-forwarding logging proxy; the endpoint DECLARED `[providers.ollama-local.compat] provider_type = \"ollama\"` exactly as the lane`s passing arm. ARM e4kcold, server COLD (`ollama ps` before the run: NO MODEL LOADED -- the default state of a stock Ollama, whose OLLAMA_KEEP_ALIVE is 5m): `/api/ps` returns `{\"models\":[]}`, `stated_window` stays None, the floor gate is blind, and turn 1 is dispatched unguarded. Proxy log, request 3: `POST /v1/chat/completions body_bytes=18723 char4_est=4680` -> `usage.prompt_tokens = 4095`. EXIT=0, a fluent answer, no refusal and no notice. POSITIVE CONTROL, same binary, same 4,217-character first prompt, cold 16,384 endpoint: `prompt_tokens = 5064`. So 5,064 real tokens were sent into a 4,096 slot and 969 of them were silently discarded FROM THE HEAD OF THE VERY FIRST REQUEST -- `4095` being slot-1, the saturation signature this issue itself reads as truncation. SECOND, SOFTER INSTANCE: arm d4k (cold, 4,096, the lane`s own task) sent one full unguarded request, `[turns: 1 | tokens: 3205 in / 503 out]`, before the turn-2 probe answered and the gate refused; the lane`s claim of ZERO chat completions holds only on a warm server. THE LANE`S OWN LOGS CONFIRM THE ASYMMETRY: /root/s2/cf-scratch/a.log line 3 shows `[(`qwen3:8b`, 4096)]` loaded BEFORE arm a ran, while its only cold arm (b) was the harmless 16,384 one. The cold x 4,096 combination was never run. HONEST CAVEAT: on a hostile-literal reading c4`s first branch survives, because a truncated Ollama prompt always reports slot-1 and 4095 < 4096 is `strictly below the slot`. That is a hole in the criterion`s wording, not a defence -- the lane`s own a0 arm calls the identical evidence shape `silently truncated`. On the criterion`s plain meaning both branches fail. WHAT WOULD UNBLOCK: when the endpoint is a declared Ollama and `/api/ps` returns no loaded model, force the load first with Ollama`s documented load-only call (`POST /api/generate` with an empty prompt -- no user data, cannot be truncated), then re-probe and only then assemble the turn; or refuse until the slot is known, which is harsher and risks c5."
  - id: c5
    text: "— NEGATIVE CONTROL: the same binary against a slot that *can* hold a turn"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::a_slot_that_can_hold_the_floor_still_completes_the_turn"
    owner: core
    note: "SECOND INSTRUMENT CONFIRMS MET, and adds the wrong-refusal control the lane did not run. I read the lane`s arm-b proxy log directly: prompt_tokens 3,193 / 10,561 / 10,631 with the last two EXCEEDING our char/4 estimates, which is the untruncated signature, and the task answered. INDEPENDENT CONTROLS, my binary (sha256 118c8466...), my endpoints: (1) at EXACTLY the 8,192 the criterion names -- a cold private Ollama whose own log reads `n_ctx_slot = 8192` and whose `/api/ps` returned 8,192 on the second probe -- the floor gate correctly ALLOWED the turn; the run was stopped later by the PRE-EXISTING #1179 learned-window refusal on an observed 3,238, not by anything this lane added, so the new gate does not turn a small-but-workable window into a refusal. (2) The same 4,217-character prompt that was truncated on the 4,096 endpoint was served IN FULL on a cold 16,384 endpoint, `prompt_tokens = 5064`, EXIT=0 -- which is simultaneously c5`s control and the positive control that makes the c4 refutation above a measurement rather than a model. Recorded for the reader: at 8,192 the 23 KB-file TASK did not complete, but that is the endpoint truncating a genuinely oversized turn (~11,825 estimated), not this fix refusing; c5`s guard clause is satisfied."
---

Created 2026-08-31 to close a COVERAGE gap, and GRADED the same day by the
`context-floor` lane of the 0.13.12 release gate. All five criteria are met;
c4 and c5 are live runs against private Ollama instances, not mocks.

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

## Second-instrument addendum (independent audit lane, 2026-08-31)

An independent instrument re-graded all five criteria from its own worktree
(`lane/f13-s2-context-floor-audit`, cut from `ca15a48bf`), its own target dir,
its own binary (sha256
`118c8466342cc5d569fab0e763785a11cab0ce58fd93613927c90f5dd5cd724e`) and its own
private Ollama instances on ports 21436 / 21437. c1, c2, c3 and c5 survive.
**c4 does not**, and is regraded `unmet`.

The disagreement is one variable: the passing c4 arm ran against a **warm**
server, and the only **cold** arm was the harmless 16,384 one. On a cold 4,096
endpoint -- the ordinary state of a stock Ollama, which unloads an idle model
after five minutes -- `/api/ps` answers `{"models":[]}`, `stated_window` stays
`None`, and turn 1 is dispatched with no guard at all. Measured: a 5,064
real-token first request (positive control on a 16,384 endpoint, same binary,
same 4,217-character prompt) reported `prompt_tokens = 4095` against the 4,096
slot, EXIT=0, fluent answer, no refusal. 969 tokens off the head of the very
first request.

Two coverage findings that are not criterion failures but should not be lost:

* Severing `probe_ollama_served_window` (returning `None` unconditionally)
  compiles and leaves **5,099 / 5,099** `wcore-agent` + `wcore-providers` tests
  green. The probe to `stated_window` link is graded by no in-tree test at all;
  only the live run covers it, and CI cannot re-run that.
* Replacing `uncompactable_floor_tokens` with a hardcoded constant compiles and
  leaves the tests c1 and c2 originally cited **green**. The c1 anchor is
  repointed here to `floor_tracks_the_tool_schemas_it_is_given`, which is the
  test that actually reds.

The decision NOT to raise `BASELINE_TURN_TOKENS` was re-derived independently
and is correct: at 8,192 the autocompact boundary is about 3,688, so a constant
of 4,636 would flip `supports_compaction(8_192)` to false and push
`minimum_workable_window` from 6,929 to roughly 10,300.

Also worth carrying to the issue: the gate arms only when the operator writes
`[providers.<alias>.compat] provider_type = "ollama"`. `docs/providers.md`
documents only the `ollama:`-model plugin route, never that knob, and the TUI
onboarding path (`persist_ollama_selection` to `render_onboarding_config`)
writes `[providers.ollama] api_key = "ollama"` with no `provider` field, which
`resolve_provider_alias` rejects outright. So no shipped, documented path
currently arms this fix. (Read from source, not executed.)

