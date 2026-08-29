---
issue: 1168
repo: FerroxLabs/wayland
title: "Turn-1 transient injection poisons the prompt-cache prefix: both fixes (system-prefix date / trailing transient message) have measured collisions"
status: closed
last_verified_commit: cfa89a9c
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
