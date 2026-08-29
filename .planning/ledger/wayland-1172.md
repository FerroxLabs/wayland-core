---
issue: 1172
repo: FerroxLabs/wayland
title: "Core cannot see a self-hosted endpoint's served context window: stock Ollama silently discards the system prompt while core reports 6% pressure"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "Core learns the window an endpoint actually serves, from the token counts already in its responses"
    state: met
    evidence: "symbol:crates/wcore-config/src/context_window.rs::ServedWindowTracker"
    owner: core
    note: "no probing and no address sniffing — the signal was already in the bytes we receive"
  - id: c2
    text: "The shortfall is named to the user, and says the HEAD of the prompt is what was lost"
    state: met
    evidence: "file:crates/wcore-config/tests/issue_1172_served_window_corpus_test.rs"
    owner: core
    note: "uses emit_info rather than warn!, so it reaches the user with RUST_LOG unset — a warn! here would have reached nobody"
  - id: c3
    text: "COMPENSATION: the learned window feeds the pre-flight guard and autocompact, so the truncation stops"
    state: not-met
    owner: core
    note: "engine.rs:12790 and :8199 still re-resolve without it. Feeding it there today would BRICK the run: at a 4,096 slot the absolute context buffers saturate to zero. Tracked as #1179"
---

Detection shipped in v0.13.10. Compensation did not — and this ticket's BODY
sets that bar, not its title.

The reporter's own words: "32,768 is still 8x the served 4,096 slot, so the
truncation persists". Core now knows the real window and says so, which turns
a silent wrong answer into a visible one. The prompt is still truncated.

c3 is blocked on a real ordering problem, not on effort: wiring the learned
window into the guard today makes a small-window run fail outright, because
the fixed buffers saturate to zero at 4,096. #1179 is that work.
