---
issue: 1210
repo: FerroxLabs/wayland
kind: defect
title: "The emergency hard-stop limit never sees the learned served window, so the reported last-resort limit can be 8x the one being enforced"
status: open
last_verified_commit: 93ae3c4a
criteria:
  - id: c1
    text: "emergency_limit_tokens resolves through the same narrowed window as resolve_preflight_window and autocompact_threshold_now, or the exemption is documented where supports_compaction is defined"
    state: met
    evidence: "symbol:crates/wcore-agent/src/compact/emergency.rs::emergency_limit"
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D28, found while verifying FerroxLabs/wayland#1172). Nothing has been done. The measured finding, verbatim: The emergency hard-stop limit is the one window-derived boundary that never sees the learned served window, so the figure core reports (and enforces) as its last-resort limit can be ~8x the window it is actually enforcing everywhere else. `emergency_limit_tokens` (crates/wcore-agent/src/engine.rs:17815) and both `is_at_emergency_limit` / `emergency_limit` call sites (engine.rs:18486, engine.rs:18497) call `emergency::emergency_limit(&self.compact_config, provider, &self.model)` (crates/wcore-agent/src/compact/emergency.rs:52), which re-resolves the window from config+model alone -- it does not go through `narrow_to_served_window` the way `resolve_preflight_window` and `autocompact_threshold_now` do. Concretely, on an unlisted model with a corroborated 8,192 learned window: enforced autocompact threshold 3,688 and pre-flight ceiling 5,053 (both narrowed), reported/enforced emergency limit 29,768 (32,768 UNVERIFIED minus the 3,000 buffer, unnarrowed). GRADED 2026-08-30 by lane w2-window-arc at 115cb4c6. emergency_limit no longer resolves a window at all: the window is an ARGUMENT, so there is no resolution left inside the function to get wrong and no second window a caller can reach. AgentEngine::emergency_limit_tokens passes compaction_window_now(), the same chokepoint resolve_preflight_window, autocompact_threshold_now and smart_compact_fraction are built on, and the run_compaction emergency check now calls emergency_limit_tokens ONCE for both the test and the number reported in AgentError::ContextTooLong -- the two call sites that used to re-derive it independently are gone. RED ARM (hetzner-dsm): restoring the config+provider+model resolution inside emergency_limit_tokens reddens c2's test; restored, touched, green."
  - id: c2
    text: "A test asserts that on a corroborated 8,192 learned window the reported emergency limit and the autocompact threshold derive from the same window"
    state: met
    evidence: "test:crates/wcore-agent/src/engine.rs::the_emergency_limit_and_the_autocompact_threshold_share_one_window"
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D28). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below. GRADED 2026-08-30 by lane w2-window-arc at 115cb4c6. Corroborates an 8,192 served window through ServedWindowTracker::observe (the Shortfall arm carries its own corroboration, so no private state is poked), asserts as a PRECONDITION that sizing_window() is Some(8_192) and that the un-narrowed window is UNVERIFIED_CONTEXT_WINDOW = 32,768, then asserts both boundaries as IDENTITIES against emergency_limit_for_window(8_192) and autocompact_threshold_for_window(8_192). Graded as identities deliberately: 'derive from the same window' is not 'smaller than 29,768', which any number of wrong windows satisfies. The measured 29,768 is asserted absent."
  - id: c3
    text: "wayland#1172 c3's ledger note stops claiming the guard, the trigger and the reported threshold cannot disagree -- or is made true"
    state: met
    evidence: "symbol:crates/wcore-agent/src/engine.rs::emergency_limit_tokens"
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D28). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below. GRADED 2026-08-30 by lane w2-window-arc at 115cb4c6. Taken on the 'or is made true' branch rather than the 'stops claiming' one: the emergency limit was the fourth window-derived boundary and it now shares compaction_window_now with the other three, so #1172 c3's note claim -- 'the guard, the trigger and the reported threshold cannot disagree' -- is true as stated instead of being narrowed to make it true. The #1172 c3 note records the fourth boundary explicitly so a reader is not left to infer the set."
---

The emergency hard-stop limit is the one window-derived boundary that never sees the learned served window, so the figure core reports (and enforces) as its last-resort limit can be ~8x the window it is actually enforcing everywhere else. `emergency_limit_tokens` (crates/wcore-agent/src/engine.rs:17815) and both `is_at_emergency_limit` / `emergency_limit` call sites (engine.rs:18486, engine.rs:18497) call `emergency::emergency_limit(&self.compact_config, provider, &self.model)` (crates/wcore-agent/src/compact/emergency.rs:52), which re-resolves the window from config+model alone -- it does not go through `narrow_to_served_window` the way `resolve_preflight_window` and `autocompact_threshold_now` do. Concretely, on an unlisted model with a corroborated 8,192 learned window: enforced autocompact threshold 3,688 and pre-flight ceiling 5,053 (both narrowed), reported/enforced emergency limit 29,768 (32,768 UNVERIFIED minus the 3,000 buffer, unnarrowed).

**Where.** crates/wcore-agent/src/engine.rs:17815 (emergency_limit_tokens), engine.rs:18486 and engine.rs:18497 (is_at_emergency_limit), reached via crates/wcore-agent/src/compact/emergency.rs:52

**Why it matters.** Impact is fail-open rather than a brick -- the narrowed pre-flight guard fires at 5,053 first, so the backstop is shadowed and no run aborts because of this. But it makes the c3 ledger note's claim false as stated: it says 'both #255 guard call sites and the autocompact trigger route through them, so the guard, the trigger and the reported threshold cannot disagree.' The emergency limit is a fourth boundary, is surfaced to operators alongside the narrowed autocompact_threshold (engine.rs:17971), and CAN disagree with it by 8x on exactly the routes #1172 is about. The deliberate-refusal comment on supports_compaction explains why the guard and trigger are gated at small windows; nothing anywhere explains why emergency is exempt, which reads as an omission rather than a decision. Worth either narrowing it on the same supports_compaction gate or writing the exemption down.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
