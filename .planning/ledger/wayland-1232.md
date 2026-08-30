---
issue: 1232
repo: FerroxLabs/wayland
kind: defect
title: "Static arms for two open-weights passthrough families served at wildly different limits by different hosts"
status: open
last_verified_commit: dba806bc4
criteria:
  - id: c1
    text: "The claim in passthrough.rs's module docs matches the table's contents"
    state: met
    evidence: "absent:crates/wcore-config/src/limits/passthrough.rs::No open-weights family is listed here"
    owner: core
    note: "CLOSED by the wayland#1176 c5 work and verified here at 1798076fe rather than inherited. The false sentence the ticket quotes -- 'No open-weights family is listed here: those live in CATALOGUE_CEILINGS, where the same rule already applies' -- is gone: grep for it returns nothing, while the known-positive control in the same call, grep -c 'open-weights' on the identical path, returns 5. So the query reads the file and the absence is real. What stands in its place is honest about the same fact: passthrough.rs:49 'OPEN WEIGHTS, STATED HONESTLY. This module used to claim \"no open-weights', :51 'and M3 rows and the DeepSeek V4 rows below are open-weights ids, served by', and :68 'Twelve of the rows below are open-weights ids. On the 2026-08-30 pull SEVEN'. The twelve-row count and the seven-violation count both match what the live gate printed today. The evidence token is an absent: anchor deliberately -- it is the only form that goes red if the claim is ever reinstated."
  - id: c2
    text: "For each of the twelve MiniMax/DeepSeek rows, a recorded decision: either the arm is removed (and the omit-max-tokens consequence stated and measured), or it is kept with the host spread written down and the reason it is safe despite the spread"
    state: met
    evidence: "file:crates/wcore-config/src/limits.rs:81:const OPEN_WEIGHTS_HOST_SPREAD: &[(&str, Option<&str>)] = &["
    owner: core
    note: "MET. The decision is recorded as a TABLE rather than as prose, so every one of the twelve rows carries its own verdict and its measured host spread: OPEN_WEIGHTS_HOST_SPREAD (limits.rs:81). All twelve arms are KEPT; the seven whose hosts disagree are PROVIDER-SCOPED to the vendor that operates them, the five whose hosts agree are left globally keyed. THE OMIT-MAX-TOKENS CONSEQUENCE, STATED AND MEASURED, because it is what makes removal wrong here rather than merely risky: an arm revokes should_omit_max_tokens, but that omission only exists when compat.omit_max_tokens_when_unsized() is true, and in this tree that is set on exactly three presets -- gemini (compat.rs:604), openrouter (:821) and flux-router (:838). deepseek_defaults() and minimax_defaults() are not among them, so on the VENDOR own API a deleted arm restores no natural ceiling at all: size_output_cap falls to UNKNOWN_CAP 8,192 and known_context_window falls to UNVERIFIED_CONTEXT_WINDOW 32,768. That is the 47x output cut wayland#1157 was filed to fix, re-introduced. Scoping delivers rule 3 intended outcome on every other route instead: None on an omit-safe reseller RESTORES the omission and the host own ceiling, and None anywhere else errs LOW at 32,768 / 8,192 -- which is the measured host FLOOR for both families (nebius serves minimax-m2.5 at 8,192 output, deepinfra serves deepseek-v4-pro at 8,192) rather than a guess. The five agreeing rows are deliberately NOT gated: at 1.0x-1.3x spread, gating them would replace a real ceiling with a low guess, which is the same harm in the other direction. Note the ROOT CAUSE this closes is the one the ticket itself names -- model_output_ceiling is keyed on the model id alone, with no provider in the key. Its provider parameter was literally _provider, accepted and unused; it is now consulted."
  - id: c3
    text: "Whatever is decided is enforced by a test that fails if a later change reverses it -- not by a comment"
    state: met
    evidence: "test:crates/wcore-config/src/limits.rs::host_variable_open_weights_arms_are_provider_scoped"
    owner: core
    note: "MET by two Rust tests plus a release-gate change, and red-armed. host_variable_open_weights_arms_are_provider_scoped fails in BOTH directions, which matters because deletion is the obvious reading of rule 3 and is the wrong one: for each of the seven it asserts the vendor endpoint still resolves the verified figures (so a DELETION reds it) and that five third-party shapes -- openrouter, flux-router, openai-compat, nebius, deepinfra -- resolve None (so a RE-GLOBALISATION reds it). It carries three controls so it cannot pass vacuously: the five agreeing rows must still resolve on every provider (a gate that scoped the whole family would fail this), claude-opus-5 must be unchanged on any provider string, and minimax-coding-plan must keep the arm while minimaxproxy must not -- provider_operates is a prefix match, not a substring one. Non-vacuity is asserted directly: the gated list must be exactly 7 and the ungated exactly 5. open_weights_rows_are_longest_fragment_first pins the ORDERING invariant structurally, because the lookup is a substring match and deepseek-v4-flash-vision-exp must be tested before deepseek-v4-flash or it inherits the gated row verdict. RED ARMS, 2026-08-30 hetzner, each cargo check exit 0 before believing the red, each restore verified by blob identity: ARM C made provider_operates return true unconditionally -- the scoping test FAILED naming deepseek-v4-flash-0731 resolved an arm on openrouter, the ordering test PASSED. ARM D swapped two ungated rows so one shadows the other, a change with IDENTICAL runtime behaviour -- the ordering test FAILED, the scoping test PASSED. Discriminating both ways. ARM E renamed the const in the Python parser regex -- the release gate self-test went to exit 1 with PARSE FAILED rather than silently reporting zero scoped arms, proving it fails CLOSED. GATE SIDE: the seven OPEN_WEIGHTS_ARM_DEBT lines are discharged, not muted. scan_open_weights_arms now takes a scoped set produced by provider_scoped_arms, which PARSES OPEN_WEIGHTS_HOST_SPREAD out of the Rust source on every run -- so un-scoping an arm in Rust makes rule 3 fail it again with no edit to the script, and the exemption cannot outlive the code that earns it. New self-test cases: a scoped arm is REPORT not FAIL, a scoped id does NOT excuse a different unscoped arm (the class-keyed-exemption trap), and the parser is checked against the real limits.rs with the agreeing rows as the control. scripts/check-model-limits-freshness.py --self-test exit 0."
---

Created 2026-08-30 by the 0.13.12 re-grade lane. This open, in-scope issue had NO ledger
file, so `scripts/check-criteria-ledger.py` was reporting it as a COVERAGE gap and nothing
graded it — while two other tickets were simultaneously blocking the release on the work it
owns.

**This ticket is the SINGLE OWNER of the twelve MiniMax/DeepSeek static arms.** Three
tickets contended for that one piece of work and the contention is resolved here:

| ticket | disposition |
|---|---|
| `wayland#1176` c5 | forward direction closed and re-verified; state corrected `not-met` → `superseded`, naming this ticket. It had a `handoff:` already, but `not-met` + `owner: core` is counted OUTSTANDING by the readiness gate whatever the handoff says, so the handoff was decorative and #1176 went on blocking for work it had handed away. |
| `wayland#1214` c1 | `superseded` onto this ticket. Its c1 and this c2 are the same work on the same twelve rows; this one states the `should_omit_max_tokens` consequence, which #1214's wording does not. #1214 c2/c3/c4 are separately closed — they were already discharged by the #1176 c5 work and nobody had re-graded them. |
| `wayland#1232` (this) | owner. c1 met, c2 and c3 outstanding. |

Criteria are the ticket's own Acceptance wording, transcribed verbatim.

`kind: defect` is deliberate and is not the safe-direction default: the arms are a live
over-claim in the `#165` direction. `model_output_ceiling` is keyed on the model id alone —
its `_provider` parameter is underscore-prefixed and unused — so a user pointing
`--model deepseek-v4-pro` at a host serving it at 128,000 is sized against 1,000,000.
