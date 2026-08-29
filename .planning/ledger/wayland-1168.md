---
issue: 1168
repo: FerroxLabs/wayland
kind: defect
title: "Turn-1 transient injection poisons the prompt-cache prefix: both fixes (system-prefix date / trailing transient message) have measured collisions"
status: closed
last_verified_commit: 3262536a
criteria:
  - id: c1
    text: "The turn-1 transient no longer lands at messages[1]; it moves into the cached system prefix"
    state: met
    evidence: "commit:6762f218"
    owner: core
    note: "Option A, landed on lane/w559; measured hit ratio 0.0358 -> 0.6526, monotonic, no late collapse"
  - id: c2
    text: "The positive control was re-anchored rather than deleted, so it still fails if the peel stops discriminating"
    state: met
    evidence: "test:crates/wcore-agent/tests/untrusted_channel_wire_test.rs::the_peel_removes_named_runtime_blocks_and_stops_on_anything_else"
    owner: core
    note: "retains a catch_unwind negative arm"
  - id: c3
    text: "The skill-router hint and PrePrompt hook contributions no longer land at messages[1] on turn 1"
    state: superseded
    owner: core
    note: "same shape as the 26-byte date that dominated the measured collapse, much smaller, not closed by this change. Carried as a criterion on #559, which is the open ticket that owns the cache-prefix outcome"
  - id: c4
    text: "The `Current date:` line the engine dispatches is today's on every turn, not the day the engine was constructed"
    state: met
    evidence: "test:crates/wcore-agent/tests/system_prompt_date_refresh.rs::a_stale_baked_date_is_refreshed_before_the_request_goes_out"
    owner: core
    note: "c1 moved the date into the cached system prefix, which bootstrap renders ONCE into a plain String; nothing re-rendered it, so the value was frozen for the life of the engine while the same prompt told the model to treat it as the authoritative today and not to substitute a different month or year. channel_dispatch holds one engine per channel session in a map with no eviction, so a gateway bot answered every date-bound question with the day it started. Graded on the WIRE (the system string the provider was handed), not on the helper"
  - id: c5
    text: "Within a day the prefix is still byte-identical across turns, so the refresh costs one cache invalidation per day and none per turn"
    state: met
    evidence: "test:crates/wcore-agent/tests/system_prompt_date_refresh.rs::a_current_date_is_not_rewritten_and_the_prefix_stays_byte_stable"
    owner: core
    note: "the control for c4. A refresher that rewrote unconditionally would bust the prefix every turn -- the exact #174 failure c1 was reverting"
---

Closed in v0.13.10 with a residual stated out loud.

A transient injected on turn 1 sat at `messages[1]`, byte 0 — inside every
provider's cached prefix — so the prefix changed on every single turn and the
prompt cache never warmed. Moving `Current date:` into the cached system
prefix took the measured hit ratio from 0.0358 to 0.6526.

Criterion c3 is `superseded`, not `met`: the same shape survives for the
skill-router hint and PrePrompt hooks. It is far smaller than the date that
dominated the collapse, and it is not closed. It is carried as a live
criterion on #559 rather than left in prose, because a residual nobody can
find is a residual nobody fixes — and the gate refuses a `superseded` whose
successor does not exist or is already closed.
