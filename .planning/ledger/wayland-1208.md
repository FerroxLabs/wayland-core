---
issue: 1208
repo: FerroxLabs/wayland
kind: defect
title: "The Current date line is frozen for the life of a session while the same prompt forbids the model correcting it"
status: closed
last_verified_commit: b692c911
criteria:
  - id: c1
    text: "A session that crosses midnight either reports the real date or stops telling the model the baked date is authoritative"
    state: met
    evidence: "symbol:crates/wcore-agent/src/context.rs::refresh_current_date_line"
    owner: core
    note: "The first branch of the disjunction is taken: the session reports the real date, and the authoritative-date sentence is kept. refresh_current_date_line rewrites the ten date bytes after the FIRST `Current date: ` occurrence when they differ from today, and is applied at the one production site that puts the prompt on the wire (engine.rs, the `LlmRequest` built in `run`). It is a pure function of (prompt, today) and returns Cow::Borrowed within a day, so the cached prefix moves once per rollover rather than once per turn. A prompt with no recognisable date line is returned untouched."
  - id: c2
    text: "The channel-gateway engine pool is covered: a long-lived per-channel engine does not answer date-bound questions with the day the gateway started"
    state: met
    evidence: "test:crates/wcore-agent/tests/issue_1208_session_day_rollover_test.rs::a_session_that_outlives_its_day_dispatches_todays_date"
    owner: core
    note: "channel_dispatch.rs serves every channel message by calling `guard.run(&prompt, &msg_id)` on the pooled per-session engine, so the pool's only contribution to the defect is making AgentEngine::run outlive its baked day; the fix sits on that dispatch. The test reproduces the pool shape -- one engine behind an Arc<Mutex<_>>, several turns, a prompt baked on 2020-03-01 -- and asserts the bytes the PROVIDER is handed, not an internal field. Positive control in the same test: a session baked TODAY dispatches a prompt that differs from the refreshed one only in the date value, so a build that rewrote the whole prompt would fail it."
  - id: c3
    text: "A test drives a day rollover against the prompt builder and asserts the outcome; current_date_present_and_stable_in_cached_system_prefix is restated to whatever the decision is rather than pinning the defect"
    state: met
    evidence: "test:crates/wcore-agent/src/context.rs::a_stale_baked_date_is_refreshed_on_the_wire"
    owner: core
    note: "a_stale_baked_date_is_refreshed_on_the_wire builds the real prefix through build_system_prompt and then asks for a different today -- exactly what a live engine sees at 00:00 -- and asserts the refreshed prompt is the baked one with the first date occurrence replaced and nothing else (same length, authority sentence intact). The pinning test is restated, not deleted: current_date_present_and_stable_in_cached_system_prefix is renamed to current_date_in_the_cached_prefix_is_stable_within_a_day_not_forever and now asserts only that the section cache does not perturb the prefix between turns, with a comment naming the two tests that carry the rollover answer. refresh_leaves_a_prompt_without_a_recognisable_date_untouched is the non-vacuity guard on the narrow rewrite."
---

The `Current date:` line is now frozen for the entire lifetime of a session and never refreshes, while the same system prompt instructs the model to treat it as authoritative and forbids correcting it: 'use the current date given above as the authoritative \'today\'. Do NOT substitute a different month or year'. `bootstrap.rs:2349` builds the prompt once into a plain `system_prompt: String` (engine.rs:3919); nothing re-renders it on a day rollover — the only rebind path is the explicit `set_system_prompt` (engine.rs:6755), which re-prepends the same baked prefix. The previous per-turn tail injection re-rendered the date every turn, so this is a behavioural regression introduced by the #1168 fix, not a pre-existing gap.

**Where.** crates/wcore-agent/src/context.rs:277-296 (frozen intro section) + crates/wcore-agent/src/bootstrap.rs:2349 (built once) + crates/wcore-agent/src/channel_dispatch.rs:89 (per-session engine pool) and :30 ('TODO(phase): (1) the engine pool is unbounded — add LRU / idle eviction')

**Why it matters.** The source comment at context.rs:272-275 waves this off as 'A long session crossing midnight therefore sees a stale date; a new session mints a new prefix' — which assumes short sessions. The channel gateway breaks that assumption by design: channel_dispatch.rs holds 'one AgentEngine per channel session' in an `Arc<Mutex<HashMap<String, Arc<Mutex<AgentEngine>>>>>` with no eviction (the TODO is unimplemented), so a Slack/Discord/Telegram bot running for a week answers every date-bound question in that conversation with the day the gateway first saw it, and is told in the same breath not to substitute a different month or year. Nothing tests day-rollover behaviour (`current_date_present_and_stable_in_cached_system_prefix` asserts the OPPOSITE — that the prefix must not change), and a search of both trackers ('stale date', 'Current date', 'midnight', 'date drift') turns up no issue for it. Cheap fix if wanted: re-render the intro when `today_string()` differs from the baked value, accepting one prefix invalidation per day per long session — the same daily cost #1168 already priced in and accepted.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
