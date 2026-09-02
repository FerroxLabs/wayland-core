---
issue: 355
repo: FerroxLabs/wayland-core
kind: defect
title: "A command-floor refusal makes the model improvise and hand the user a confident wrong answer instead of blocked by policy"
status: closed
last_verified_commit: d647fbba
criteria:
  - id: c1
    text: "A floor refusal is distinguishable by the model from a transient tool failure, carrying a marker that says this is a policy decision"
    state: met
    owner: core
    evidence: "symbol:crates/wcore-config/src/command_floor.rs::POLICY_REFUSAL_MARKER"
    note: "Every refusal now leaves `floor_refusal` through `disclose`, which appends the marker `[POLICY-REFUSAL: command-floor]` to the payload. `is_policy_refusal` is the single predicate any surface asks, and it matches the MARKER rather than the refusal prose so the prose stays free to change. Polarity is graded both ways: `every_refusal_carries_the_marker_and_the_stop_instruction` over all three rule families, and `a_transient_failure_is_not_a_policy_refusal` plus the e2e control `an_ordinary_command_failure_is_not_dressed_as_a_policy_refusal`, which proves a command-not-found is NOT dressed as policy. A marker fired on every error would distinguish nothing."
  - id: c2
    text: "The refusal instructs the model to surface it to the user rather than work around it, and says so in the PAYLOAD rather than a log line"
    state: met
    owner: core
    evidence: "symbol:crates/wcore-config/src/command_floor.rs::DISCLOSURE_DIRECTIVE"
    note: "The directive is concatenated into the refusal STRING the tool returns, so it lands in the tool_result payload the model reads on every one of the five call sites (four `BashTool` entry points plus `wcore_skills::shell`) with no per-site change. No log line is involved anywhere in this change: RUST_LOG is unset on a default install so only ERROR reaches stderr, and the model never reads the log under any setting. It names the specific workarounds the report saw - retry, respell the path, stage in a temp directory and move it in - because 'stop' alone left 'try another way' looking reasonable. Asserted in the payload end to end by `a_floor_refusal_payload_marks_itself_a_policy_decision_and_says_stop`."
  - id: c3
    text: "A test drives a real floor refusal end to end and asserts the USER-VISIBLE output names the refusal"
    state: met
    owner: core
    evidence: "test:crates/wcore-agent/tests/floor_refusal_reaches_the_user.rs::a_floor_refusal_reaches_the_user_even_when_the_model_improvises"
    note: "Real `BashTool` + real `AgentEngine` + real `ProtocolSink`, asserting on the JSON Lines a host renders. The trip is a genuine rule-1 refusal (`cat .git/config`), not a stub. The scripted model improvises exactly as the report describes and STILL the user is told, because the disclosure the assertion reads is the engine notice (`emit_info` -> terminal session line / `ProtocolEvent::Info` -> TUI `push_system` and json-stream hosts), which does not depend on the model complying. `user_visible()` deliberately EXCLUDES the `[Bash error] ...` relay frame that `ProtocolSink::emit_tool_result` writes: that frame is the model's own input echoed for display, it is what the shipped tree already had, and grading against it is the mistake the previous note calls out. The control arm proves the notice fires on policy only."
  - id: c4
    text: "A red arm is quoted verbatim, reproducing the improvisation"
    state: met
    owner: core
    evidence: "commit:98088741"
    note: "The test was committed BEFORE the fix (98088741) so the reproduction is a commit in the tree rather than a transcript claim, then the fix landed on top (d647fbba). Run at 98088741 the payload arm failed with the payload verbatim: `[Bash error] Refused by the command floor: this command references the repository control surface (.git/hooks, .git/config, .wayland-core). ...` - no marker, indistinguishable from a failed command. The disclosure arm failed with the whole user-visible transcript verbatim, quoted in full in the body below: three pricing/tool-call notices and then `[assistant] I staged the skill under /tmp, wrote it to the destination and cd'd in. The brief is set up.` - the incident, with no mention of policy anywhere. Same three tests at d647fbba: 3 passed, 0 skipped."
---

The more dangerous half of the command-floor over-refusal report, and independent
of it: once the floor stops over-refusing, a floor refusal in any OTHER situation
will still fail this way.

Refused in place, the model improvised — staged the skill under `/tmp`, wrote a
file at the destination, `cd`'d into it, and then told the user it could not run
the brief. The user did not see "blocked by policy". They saw a confident wrong
answer plus side effects nobody asked for and nobody was told about.

An un-liftable guard is only as good as the model's willingness to stop when it
fires. The floor is not the only un-liftable guard in the product, so this is a
class defect, not a floor defect.

UPDATE 2026-08-29: the rule-1 over-refusal hotfix HAS now landed in `integ/next`
(20d99006, graded by `skill_scripts_under_wayland_core_are_runnable` with the
control `the_wayland_core_control_surface_stays_refused`). That removes the
reported instance of the trigger and changes nothing about this ticket: the
behaviour under any other floor refusal is untouched, which is exactly why this
was filed on its own.

## The red arm, verbatim (c4)

Reproduced on `lane/f13-floor-disclosure` at `98088741` — the test present, the
fix absent — with
`cargo nextest run -p wcore-agent --test floor_refusal_reaches_the_user`:

```
thread 'a_floor_refusal_payload_marks_itself_a_policy_decision_and_says_stop' (2456044) panicked at crates/wcore-agent/tests/floor_refusal_reaches_the_user.rs:203:5:
the refusal the model reads must carry the policy marker `[POLICY-REFUSAL: command-floor]`, otherwise it is indistinguishable from a missing binary or a flaky sandbox. Payload was:
[Bash error] Refused by the command floor: this command references the repository control surface (.git/hooks, .git/config, .wayland-core). Writing there is arbitrary code execution as you on your next git command, so it is refused below approval and --force alike. Ordinary git work (add, commit, status, push) is unaffected.
```

```
thread 'a_floor_refusal_reaches_the_user_even_when_the_model_improvises' (2456204) panicked at crates/wcore-agent/tests/floor_refusal_reaches_the_user.rs:238:5:
the user must be told a POLICY blocked the command, not left with the model's improvised answer. User-visible output was:
[notice] No published price for anthropic/test-model; the pre-flight reservation uses the anthropic list rate as a conservative ceiling. Spend is reported only where a real price is known.
[notice] Tool call: Bash
[notice] No published price for anthropic/test-model; the pre-flight reservation uses the anthropic list rate as a conservative ceiling. Spend is reported only where a real price is known.
[assistant] I staged the skill under /tmp, wrote it to the destination and cd'd in. The brief is set up.
```

```
     Summary [   0.250s] 3 tests run: 1 passed, 2 failed, 0 skipped
```

That is the incident: the transcript the user reads names no policy at all, and
the last thing in it is the model's confident account of the workaround. The one
arm that passed is the control (`an_ordinary_command_failure_is_not_dressed_as_a_policy_refusal`),
which is supposed to pass on both sides — it is what stops the disclosure from
being a notice fired on every error.

The green arm, same three tests at `d647fbba`:

```
        PASS [   0.124s] (1/3) wcore-agent::floor_refusal_reaches_the_user a_floor_refusal_payload_marks_itself_a_policy_decision_and_says_stop
        PASS [   0.127s] (2/3) wcore-agent::floor_refusal_reaches_the_user a_floor_refusal_reaches_the_user_even_when_the_model_improvises
        PASS [   0.130s] (3/3) wcore-agent::floor_refusal_reaches_the_user an_ordinary_command_failure_is_not_dressed_as_a_policy_refusal
────────────
     Summary [   0.130s] 3 tests run: 3 passed, 0 skipped
```

## Scope note

The disclosure notice is raised at the ONE site in `wcore-agent::engine` where
every tool result reaches the user sink, keyed off `is_policy_refusal`. It is
therefore not floor-specific by construction: any other un-liftable guard that
adopts the marker gets the same disclosure for free, which is the class half of
this ticket. `crates/wcore-agent/src/engine.rs` is a Desktop-contract
`SOURCE_INPUTS` file, so the corpus was re-pinned in the same change —
`schema_digest` unchanged, only `source_inputs_digest` and `fixture_digest`
moved.
