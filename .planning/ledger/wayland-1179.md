---
issue: 1179
repo: FerroxLabs/wayland
title: "Absolute context buffers saturate to zero on a small served window, so a learned window cannot be used to compact"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "input_ceiling() returns a positive value on a small learned or configured window instead of saturating to zero"
    state: not-met
    owner: core
    note: "today output_reserve 20000 plus emergency_buffer 3000 exceeds a 4096 window, so the #255 guard would abort every turn"
  - id: c2
    text: "The autocompact threshold on a small window sits above core's own baseline turn rather than below it"
    state: not-met
    owner: core
    note: "0.70 x 4096 is 2867, under the measured 3118-token baseline turn — an LLM summarization at the top of every turn forever"
  - id: c3
    text: "Behaviour is measured at the 4k, 8k, 32k, 60k and 200k window points rather than derived from a chosen fraction"
    state: not-met
    owner: core
  - id: c4
    text: "A test at each of those points distinguishes compacts usefully from fires every turn"
    state: not-met
    owner: core
  - id: c5
    text: "The 33k-110k band does not regress: a pinned 60000 window keeps a threshold below its own pre-flight shed ceiling"
    state: not-met
    owner: core
    note: "a naive proportional floor moves 60k from 27000 to 42000, past a shed ceiling of 37000; nothing in #1150 is evidence about this band"
---

#1172 taught core to learn an endpoint's genuinely-served context window from
`usage.prompt_tokens`. That learned figure deliberately does not feed the #255
pre-flight guard or the compaction thresholds, because the buffers are absolute
and were tuned when the only window in play was 200,000. At 4,096 they brick the
run rather than save it: the input ceiling saturates to zero and the autocompact
threshold falls below core's own baseline turn.

So the learned window today drives only the user-facing notice and the pressure
gauge. This issue is the compensation half, and it is the reason #1172's own c3
is not met.

The scope is narrower than it looks. At 4,096 no compaction strategy can work at
all — the honest remedy is the operator raising the server's context length,
which the notice now says. This is about the band where a small window is still
workable, roughly 8k to 32k. c5 is here because the obvious fix has a known way
of making a different band worse, and that band has never been measured.
