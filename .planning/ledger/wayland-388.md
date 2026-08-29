---
issue: 388
repo: FerroxLabs/wayland
kind: defect
title: "[Bug]: Long-running tasks intermittently truncate, stall, or restart inconsistently through Free Models Router"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "Output caps are decided from what is actually known about the served model, not from the alias the request named"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c2
    text: "Reasoner replay is decided the same way, from what is known rather than from the alias"
    state: met
    evidence: "commit:0cab1cf8"
    owner: core
  - id: c3
    text: "A prompt silently discarded by an under-sized served window is named to the user rather than showing as low pressure"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::ServedWindowTracker"
    owner: core
    note: "shipped under #1172; a different cause of the same reported symptom"
  - id: c4
    text: "The remaining four bullets of this ticket's own Expected Behavior list are met"
    state: not-met
    owner: core
    note: "RE-OWNED TO CORE 2026-08-29. The previous note said these bullets are router-side retry/stall behaviour core cannot change from this repo. Graded against the ticket's own Expected Behavior list, that is wrong. Core met the two exposure bullets (which model/provider failed; which of the five failure classes it was) plus the served-window disclosure. The four that remain are: stop cleanly before any write operations; preserve a checkpoint and allow the user to continue; clearly mark the task as failed/incomplete; prevent partial commits or speculative file changes after truncation. Every one of those is agent-harness behaviour in wcore-agent, not routing: StopReason::MaxTokens is core's own concept (crates/wcore-agent/src/provider_recovery.rs:598) and the checkpoint machinery is core's journal and session_recovery_replay. The reporter says so themself in Additional Context: even if Free Models Router is less reliable, Wayland should fail safely and visibly rather than leaving the user unsure whether a repo task completed. No flux change can deliver that. Nothing is handed out; this is core work not yet done."
---

Graded against this ticket's own Expected Behavior list: 3 of 7 bullets are
met at v0.13.10, which is why it stays open.

Core's half was that output caps and reasoner replay were being decided from
`request.model` — the alias the caller typed — rather than from the model the
router actually served. `0cab1cf8` decides both from what is known.

RE-GRADED 2026-08-29. c4 was `blocked owner=flux` on the reading that the rest
is the router's side of the same symptom. Against the ticket's own Expected
Behavior list that is wrong. The four remaining bullets are: stop cleanly
before any write operations; preserve a checkpoint and allow the user to
continue; clearly mark the task as failed/incomplete; prevent partial commits
or speculative file changes after truncation. Not one of those is routing --
they are all agent-harness behaviour in wcore-agent, and the reporter frames
them that way themself: even if the Free Models Router is expected to be less
reliable, Wayland should fail safely and visibly rather than leave the user
unsure whether a repo task completed. No flux change can deliver that, so c4
is core's and is now owned as such rather than handed out.

#1172 closed a third, independent cause of the same user-visible complaint (an
endpoint silently discarding the prompt), which is worth reading alongside this
before anyone re-grades it again.
