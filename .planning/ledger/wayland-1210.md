---
issue: 1210
repo: FerroxLabs/wayland
kind: defect
title: "The emergency hard-stop limit never sees the learned served window, so the reported last-resort limit can be 8x the one being enforced"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "emergency_limit_tokens resolves through the same narrowed window as resolve_preflight_window and autocompact_threshold_now, or the exemption is documented where supports_compaction is defined"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D28, found while verifying FerroxLabs/wayland#1172). Nothing has been done. The measured finding, verbatim: The emergency hard-stop limit is the one window-derived boundary that never sees the learned served window, so the figure core reports (and enforces) as its last-resort limit can be ~8x the window it is actually enforcing everywhere else. `emergency_limit_tokens` (crates/wcore-agent/src/engine.rs:17815) and both `is_at_emergency_limit` / `emergency_limit` call sites (engine.rs:18486, engine.rs:18497) call `emergency::emergency_limit(&self.compact_config, provider, &self.model)` (crates/wcore-agent/src/compact/emergency.rs:52), which re-resolves the window from config+model alone -- it does not go through `narrow_to_served_window` the way `resolve_preflight_window` and `autocompact_threshold_now` do. Concretely, on an unlisted model with a corroborated 8,192 learned window: enforced autocompact threshold 3,688 and pre-flight ceiling 5,053 (both narrowed), reported/enforced emergency limit 29,768 (32,768 UNVERIFIED minus the 3,000 buffer, unnarrowed)."
  - id: c2
    text: "A test asserts that on a corroborated 8,192 learned window the reported emergency limit and the autocompact threshold derive from the same window"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D28). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "wayland#1172 c3's ledger note stops claiming the guard, the trigger and the reported threshold cannot disagree -- or is made true"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D28). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The emergency hard-stop limit is the one window-derived boundary that never sees the learned served window, so the figure core reports (and enforces) as its last-resort limit can be ~8x the window it is actually enforcing everywhere else. `emergency_limit_tokens` (crates/wcore-agent/src/engine.rs:17815) and both `is_at_emergency_limit` / `emergency_limit` call sites (engine.rs:18486, engine.rs:18497) call `emergency::emergency_limit(&self.compact_config, provider, &self.model)` (crates/wcore-agent/src/compact/emergency.rs:52), which re-resolves the window from config+model alone -- it does not go through `narrow_to_served_window` the way `resolve_preflight_window` and `autocompact_threshold_now` do. Concretely, on an unlisted model with a corroborated 8,192 learned window: enforced autocompact threshold 3,688 and pre-flight ceiling 5,053 (both narrowed), reported/enforced emergency limit 29,768 (32,768 UNVERIFIED minus the 3,000 buffer, unnarrowed).

**Where.** crates/wcore-agent/src/engine.rs:17815 (emergency_limit_tokens), engine.rs:18486 and engine.rs:18497 (is_at_emergency_limit), reached via crates/wcore-agent/src/compact/emergency.rs:52

**Why it matters.** Impact is fail-open rather than a brick -- the narrowed pre-flight guard fires at 5,053 first, so the backstop is shadowed and no run aborts because of this. But it makes the c3 ledger note's claim false as stated: it says 'both #255 guard call sites and the autocompact trigger route through them, so the guard, the trigger and the reported threshold cannot disagree.' The emergency limit is a fourth boundary, is surfaced to operators alongside the narrowed autocompact_threshold (engine.rs:17971), and CAN disagree with it by 8x on exactly the routes #1172 is about. The deliberate-refusal comment on supports_compaction explains why the guard and trigger are gated at small windows; nothing anywhere explains why emergency is exempt, which reads as an omission rather than a decision. Worth either narrowing it on the same supports_compaction gate or writing the exemption down.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
