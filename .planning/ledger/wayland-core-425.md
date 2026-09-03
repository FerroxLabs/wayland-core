---
issue: 425
repo: FerroxLabs/wayland-core
kind: defect
title: "PreparedContentBlockV1::Thinking.extra still uses the pre-23B-H1 Option::is_none skip guard, re-opening the null-collapse class in the provider-request digest preimage"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "A test builds an LlmRequest whose assistant message carries ContentBlock::Thinking with extra = Some(Value::Null), snapshots it and decodes it. On unmodified 6e4eca07 the decode returns Err whose message is exactly 'prepared provider request snapshot is not canonical' -- asserted on the string, not merely on Err. After the fix it returns Ok. The failing pre-fix run is recorded with its error text."
    state: not-met
    owner: core
    note: "Filed 2026-09-03. Anchored at 6e4eca07, which does NOT carry the fix, so every criterion is not-met here by construction. RED ARM ALREADY RUN, by reverting only the predicate to Option::is_none on the fix branch: `an explicit null extra must survive recovery: invalid journal state transition: prepared provider request snapshot is not canonical` -- the predicted string verbatim. GREEN ARM: 5 passed / 0 failed. The red arm carries its own control: a_tool_use_block_with_an_explicit_null_extra_still_decodes stays OK in the red arm, because ToolUse.extra has no skip_serializing_if at all -- so the red is attributable to the predicate and not to the test harness. Flip with a post-merge sync anchored at the merge commit."
  - id: c2
    text: "The fix is the PREDICATE, not the caller: the skip_serializing_if on the Thinking arm no longer names Option::is_none but a predicate true for both None and Some(Value::Null), and to_value -> from_value -> to_value is byte-identical at extra = None, Some(Null) and Some(object). Rejecting a Some(Null) at the writer does NOT satisfy this."
    state: not-met
    owner: core
    note: "Implemented as is_absent_or_explicitly_null, deliberately NOT by reusing model::is_absent_json_value: that one consults LEGACY_EFFECT_RECEIPT_ENCODING so pre-23B-H1 effect_receipt bytes can still be read, and sharing it would make this field's encoding silently revert whenever a legacy receipt is being decoded. The bijection is pinned by a_thinking_block_re_encodes_identically_at_every_shape_of_extra, which fails on the pre-fix predicate at the Some(Null) shape."
  - id: c3
    text: "No already-written digest moves: the two currently-reachable shapes encode byte-identically before and after, so no journal on disk becomes unreadable. The existing wayland#1170 backward-compat test passes unmodified."
    state: not-met
    owner: core
    note: "prepared_provider_request_snapshot_keeps_a_thinking_block_without_extra_byte_identical is untouched and passes in BOTH arms -- green and red -- which is exactly the shape of evidence this criterion wants: the fix is invisible to the shapes that already exist on disk."
  - id: c4
    text: "No legacy-recovery shim is added, and the reason is proven rather than asserted: every non-test construction site of ContentBlock::Thinking either sets extra: None or builds it via a map that can only yield a JSON object. If any site can yield Some(Value::Null), this criterion FAILS and the ticket escalates to a live defect needing a read-path shim."
    state: not-met
    owner: core
    note: "This criterion also disposed of a mistake in the first cut of the fix's own test, which asserted the decoded message equalled the input -- i.e. that Some(Null) is PRESERVED. It is not, and must not be: preserving it is what would require the shim this criterion forbids. The test now asserts the collapse explicitly (extra decodes as None, and the field never reaches the wire) so a change that quietly starts preserving it has to come past those lines."
  - id: c5
    text: "Reachability is stated honestly and not graded by a run that cannot exist. This is LATENT: no producer emits Some(Value::Null) at 6e4eca07, so there is no live end-to-end reproduction and none may be claimed. Any text asserting a user-visible recovery failure fails this criterion."
    state: not-met
    owner: core
    note: "The single production producer builds extra with a map that yields an object or None. What justifies the ticket is that the guard against repeating a known permanent-data-loss defect (23B-01: journals written successfully, never readable again) was the SHAPE of one map at one call site, with no test on the null arm. The three tests are that guard."
  - id: c6
    text: "Class sweep, pinned so it can fail later: every Option<serde_json::Value> field carrying a skip_serializing_if whose value reaches a state_payload_digest preimage is enumerated, and each is either null-collapsing or carries an inline comment saying why Option::is_none is safe there. On this tree that is THREE production hits, all in the session journal: reducer.rs PreparedContentBlockV1::Thinking.extra, model.rs:745 and model.rs:1131 (both effect_receipt). A sweep that does not name all three does not satisfy this. CORRECTED 2026-09-04: as filed this criterion named a fourth site, model.rs:1133, and required all four. That attribute governs pre_hook_phase_id: Option<String>, which cannot hold Value::Null and so is not in the class this criterion defines -- a correct sweep can never name it, and c6 as filed could not pass."
    state: not-met
    owner: core
    note: "NOT DONE BY THE FIRST PR, deliberately and stated rather than quietly folded in: the PR closes the one hit that was wrong (Thinking.extra) and leaves the sweep as remaining work. model.rs:745 and :1131 already use is_absent_json_value and are the 23B-H1 remedy sites, so the expected outcome is that they pass -- but 'expected to pass' is not a graded sweep, and recording it as met on that basis is the exact failure this ledger exists to catch."
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
