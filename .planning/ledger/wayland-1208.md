---
issue: 1208
repo: FerroxLabs/wayland
kind: defect
title: "The Current date line is frozen for the life of a session while the same prompt forbids the model correcting it"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "A session that crosses midnight either reports the real date or stops telling the model the baked date is authoritative"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D26, found while verifying wayland#1168). Nothing has been done. The measured finding, verbatim: The `Current date:` line is now frozen for the entire lifetime of a session and never refreshes, while the same system prompt instructs the model to treat it as authoritative and forbids correcting it: 'use the current date given above as the authoritative \'today\'. Do NOT substitute a different month or year'. `bootstrap.rs:2349` builds the prompt once into a plain `system_prompt: String` (engine.rs:3919); nothing re-renders it on a day rollover — the only rebind path is the explicit `set_system_prompt` (engine.rs:6755), which re-prepends the same baked prefix. The previous per-turn tail injection re-rendered the date every turn, so this is a behavioural regression introduced by the #1168 fix, not a pre-existing gap."
  - id: c2
    text: "The channel-gateway engine pool is covered: a long-lived per-channel engine does not answer date-bound questions with the day the gateway started"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D26). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test drives a day rollover against the prompt builder and asserts the outcome; current_date_present_and_stable_in_cached_system_prefix is restated to whatever the decision is rather than pinning the defect"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D26). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The `Current date:` line is now frozen for the entire lifetime of a session and never refreshes, while the same system prompt instructs the model to treat it as authoritative and forbids correcting it: 'use the current date given above as the authoritative \'today\'. Do NOT substitute a different month or year'. `bootstrap.rs:2349` builds the prompt once into a plain `system_prompt: String` (engine.rs:3919); nothing re-renders it on a day rollover — the only rebind path is the explicit `set_system_prompt` (engine.rs:6755), which re-prepends the same baked prefix. The previous per-turn tail injection re-rendered the date every turn, so this is a behavioural regression introduced by the #1168 fix, not a pre-existing gap.

**Where.** crates/wcore-agent/src/context.rs:277-296 (frozen intro section) + crates/wcore-agent/src/bootstrap.rs:2349 (built once) + crates/wcore-agent/src/channel_dispatch.rs:89 (per-session engine pool) and :30 ('TODO(phase): (1) the engine pool is unbounded — add LRU / idle eviction')

**Why it matters.** The source comment at context.rs:272-275 waves this off as 'A long session crossing midnight therefore sees a stale date; a new session mints a new prefix' — which assumes short sessions. The channel gateway breaks that assumption by design: channel_dispatch.rs holds 'one AgentEngine per channel session' in an `Arc<Mutex<HashMap<String, Arc<Mutex<AgentEngine>>>>>` with no eviction (the TODO is unimplemented), so a Slack/Discord/Telegram bot running for a week answers every date-bound question in that conversation with the day the gateway first saw it, and is told in the same breath not to substitute a different month or year. Nothing tests day-rollover behaviour (`current_date_present_and_stable_in_cached_system_prefix` asserts the OPPOSITE — that the prefix must not change), and a search of both trackers ('stale date', 'Current date', 'midnight', 'date drift') turns up no issue for it. Cheap fix if wanted: re-render the intro when `today_string()` differs from the baked value, accepting one prefix invalidation per day per long session — the same daily cost #1168 already priced in and accepted.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
