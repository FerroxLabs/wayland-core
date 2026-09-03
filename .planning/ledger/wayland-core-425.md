---
issue: 425
repo: FerroxLabs/wayland-core
kind: defect
title: "PreparedContentBlockV1::Thinking.extra still uses the pre-23B-H1 Option::is_none skip guard, re-opening the null-collapse class in the provider-request digest preimage"
status: open
last_verified_commit: fb6f43dd7
criteria:
  - id: c1
    text: "A test builds an LlmRequest whose assistant message carries ContentBlock::Thinking with extra = Some(Value::Null), snapshots it and decodes it. On unmodified 6e4eca07 the decode returns Err whose message is exactly 'prepared provider request snapshot is not canonical' -- asserted on the string, not merely on Err. After the fix it returns Ok. The failing pre-fix run is recorded with its error text."
    state: met
    evidence: "test:crates/wcore-agent/tests/session_journal_test/foundation_cases.rs::a_thinking_block_with_an_explicit_null_extra_still_decodes"
    owner: core
    note: "MET at fb6f43dd7. BOTH ARMS RUN, with a control. RED ARM at 774c40f5a -- that tree is 6e4eca07 plus the train's earlier merges and still carries the unmodified `skip_serializing_if = 'Option::is_none'` on the Thinking arm (verified by grep: 0 hits for is_absent_or_explicitly_null). The post-fix tests were checked out onto it WITHOUT the reducer.rs fix, so only the guard differs. Two failed, verbatim: `an explicit null extra must survive recovery: invalid journal state transition: prepared provider request snapshot is not canonical` (foundation_cases.rs:356) and `extra=Some(Null) did not decode: invalid journal state transition: prepared provider request snapshot is not canonical` (foundation_cases.rs:395). THE TEST WAS STRENGTHENED TO EARN THIS. As first written it used `unwrap_or_else(|error| panic!("...: {error}"))`, which INTERPOLATES the error into a panic message and asserts nothing -- an unrelated decode failure would have failed it identically, which is the exact distinction c1 exists to force. It now matches both outcomes: a fixed tree takes the Ok arm and never runs the assertion; the pre-fix predicate takes the Err arm, runs `assert_eq!(error.to_string(), ...)` against the full rendered message, and only then fails. Confirmed by frame, not by inference: the red-arm panic is at foundation_cases.rs:373, the `panic!` AFTER the assert_eq at 366-372, so the equality assertion ran and matched. c1 quotes only the inner phrase; JournalError's Display prefixes `invalid journal state transition: `, and the test asserts the whole rendered string, which is strictly stronger than matching the fragment. CROSS-AUDITED: Codex 5.6 Sol and Gemini 3.1 Pro independently graded the original test not-met against c1, and both rejected a hand-built-snapshot alternative as measuring deserializer strictness rather than the round-trip defect. Kimi K3 was unavailable (quota) -- a two-engine check, not three. CONTROL: a_tool_use_block_with_an_explicit_null_extra_still_decodes PASSES in the red arm too, because ToolUse.extra carries no skip_serializing_if -- so the red arm discriminates the defect and is not just a tree that fails. GREEN ARM: all three PASS on the fixed tree (nextest 3926-3928 of 17821)."
  - id: c2
    text: "The fix is the PREDICATE, not the caller: the skip_serializing_if on the Thinking arm no longer names Option::is_none but a predicate true for both None and Some(Value::Null), and to_value -> from_value -> to_value is byte-identical at extra = None, Some(Null) and Some(object). Rejecting a Some(Null) at the writer does NOT satisfy this."
    state: met
    evidence: "symbol:crates/wcore-agent/src/session_journal/reducer.rs::is_absent_or_explicitly_null"
    owner: core
    note: "MET at fb6f43dd7. The predicate, not the caller: reducer.rs now reads `#[serde(default, skip_serializing_if = 'is_absent_or_explicitly_null')]`, and the fn is `matches!(value, None | Some(serde_json::Value::Null))` -- true for both, as required. The three-shape round trip is a_thinking_block_re_encodes_identically_at_every_shape_of_extra, PASS on the fixed tree and FAIL pre-fix at the Some(Null) arm specifically."
  - id: c3
    text: "No already-written digest moves: the two currently-reachable shapes encode byte-identically before and after, so no journal on disk becomes unreadable. The existing wayland#1170 backward-compat test passes unmodified."
    state: met
    evidence: "test:crates/wcore-agent/tests/session_journal_test/foundation_cases.rs::prepared_provider_request_snapshot_keeps_a_thinking_block_without_extra_byte_identical"
    owner: core
    note: "MET at fb6f43dd7. The wayland#1170 backward-compat test is UNMODIFIED: the whole change to foundation_cases.rs is one purely additive hunk, `@@ -329,6 +329,92 @@`, entirely after that test, which begins at line 311. It PASSES on the fixed tree (nextest 3960 of 17821)."
  - id: c4
    text: "No legacy-recovery shim is added, and the reason is proven rather than asserted: every non-test construction site of ContentBlock::Thinking either sets extra: None or builds it via a map that can only yield a JSON object. If any site can yield Some(Value::Null), this criterion FAILS and the ticket escalates to a live defect needing a read-path shim."
    state: met
    evidence: "absent:crates/wcore-agent/src/session_journal/reducer.rs::restore_explicit_null_receipt"
    owner: core
    note: "MET at fb6f43dd7. Enumerated, not asserted. Every `ContentBlock::Thinking` occurrence outside tests/ was classified; the enumeration is complete because no occurrence puts the brace on a following line (the remaining bare mentions are all comments or doc comments). Non-test CONSTRUCTION sites: protocol_bridge.rs:2820, openai.rs:4180, openai.rs:5344, openai.rs:5393 and engine.rs:25688 all set `extra: None`. The ONLY site with a computed value is engine.rs:17009: `extra: thinking_signature.take().map(|sig| serde_json::json!({ 'thoughtSignature': sig }))`, where `thinking_signature: Option<String>` (engine.rs:14648), so the map yields None or a JSON OBJECT with a fixed key and can never yield Some(Value::Null). No read-path shim is owed."
  - id: c5
    text: "Reachability is stated honestly and not graded by a run that cannot exist. This is LATENT: no producer emits Some(Value::Null) at 6e4eca07, so there is no live end-to-end reproduction and none may be claimed. Any text asserting a user-visible recovery failure fails this criterion."
    state: met
    evidence: "absent:crates/wcore-agent/src/engine.rs::extra: Some(serde_json::Value::Null)"
    owner: core
    note: "MET at fb6f43dd7, and met by being HONEST rather than by a run. This is LATENT: c4's enumeration shows no producer can emit Some(Value::Null), so no live end-to-end reproduction exists and none is claimed anywhere in this ledger or in the PR. The defect is real in the type's domain and unreachable from today's producers; the fix closes it before a producer makes it reachable."
  - id: c6
    text: "Class sweep, pinned so it can fail later: every Option<serde_json::Value> field carrying a skip_serializing_if whose value reaches a state_payload_digest preimage is enumerated, and each is either null-collapsing or carries an inline comment saying why Option::is_none is safe there. On this tree that is THREE production hits, all in the session journal: reducer.rs PreparedContentBlockV1::Thinking.extra, model.rs:745 and model.rs:1131 (both effect_receipt). A sweep that does not name all three does not satisfy this. CORRECTED 2026-09-04: as filed this criterion named a fourth site, model.rs:1133, and required all four. That attribute governs pre_hook_phase_id: Option<String>, which cannot hold Value::Null and so is not in the class this criterion defines -- a correct sweep can never name it, and c6 as filed could not pass."
    state: met
    evidence: "symbol:crates/wcore-agent/src/session_journal/model.rs::is_absent_json_value"
    owner: core
    note: "MET at fb6f43dd7. Sweep run workspace-wide, not only in the two files the criterion named. Class = `Option<serde_json::Value>` carrying a `skip_serializing_if` and reaching a state_payload_digest preimage. THREE production hits, all now null-collapsing: reducer.rs:2058 Thinking.extra (is_absent_or_explicitly_null) and effect_receipt at model.rs:745 and model.rs:1131 (both is_absent_json_value). The only other Option<serde_json::Value> fields carrying the attribute anywhere in crates/ are two in wcore-acp (protocol.rs params, result); wcore-acp is NOT a dependency of wcore-agent and is never referenced from session_journal, so neither can reach the preimage. The criterion as filed named a fourth site, model.rs:1133, which governs pre_hook_phase_id: Option<String> and is not in the class -- see the CORRECTED clause in the criterion text."
---

# The absence of the attribute is what made the sibling safe

`ToolUse.extra` survives `Some(Value::Null)` today, and not because anyone
handled it. It survives because it carries no `skip_serializing_if` at all, so
its encoding is already a bijection. `Thinking.extra` carried one, and the
predicate it named collapsed only half of the values that must collapse.

The failure that follows is not a lost field. `decode_prepared_provider_request_snapshot`
re-encodes and compares for exact equality, so any divergence becomes a
permanent `InvalidTransition` refusal of recovery -- the write succeeds and the
conversation is never readable again. That check is the blast radius, not the
shield.
