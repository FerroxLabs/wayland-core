---
issue: 378
repo: FerroxLabs/wayland-core
kind: defect
title: "spill_readback_engine_wiring is the slowest test in wcore-agent and was killed at 60.010s on an otherwise-completing run"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The runtime is ATTRIBUTED by measurement: whether the >30s is product latency on the spill/read-back path or the fixture's own cost, established rather than inferred"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D4, found while verifying wayland-core#337). Nothing has been done. The measured finding, verbatim: `wcore-agent::spill_readback_engine_wiring::the_engine_spills_where_this_session_can_read_it_back` is the slowest test in the crate and is not in the flaky allowlist. It reported SLOW [>30.000s] in all three of my full-suite runs and in one of them was killed: `TIMEOUT [ 60.010s] (3894/3895)`. That run finished `3894 passed, 1 timed out`. The other two runs it completed. Caveat stated up front so this is not oversold: I ran the `default` nextest profile (slow-timeout 30s, terminate-after 2 -> kill at 60s, matching the observed 60.010s). `[profile.ci]` gives 90s x 2 = 180s, so CI would probably not kill it -- but it is the one test in this crate whose runtime is load-sensitive enough to cross a kill budget at all, it is unlisted, and with grade-retry-flakes.sh gating `report` an unlisted flake fails the run."
  - id: c2
    text: "Either the test completes inside the default profile's 30s slow-timeout, or it carries an explicit per-test budget with the reason recorded in .config/nextest.toml"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D4). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "If it stays load-sensitive it is listed in .config/flaky-allowlist.txt with an expiry and a measured rate, so an unlisted flake cannot redden the required report check with no diagnostic attached"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D4). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

`wcore-agent::spill_readback_engine_wiring::the_engine_spills_where_this_session_can_read_it_back` is the slowest test in the crate and is not in the flaky allowlist. It reported SLOW [>30.000s] in all three of my full-suite runs and in one of them was killed: `TIMEOUT [ 60.010s] (3894/3895)`. That run finished `3894 passed, 1 timed out`. The other two runs it completed. Caveat stated up front so this is not oversold: I ran the `default` nextest profile (slow-timeout 30s, terminate-after 2 -> kill at 60s, matching the observed 60.010s). `[profile.ci]` gives 90s x 2 = 180s, so CI would probably not kill it -- but it is the one test in this crate whose runtime is load-sensitive enough to cross a kill budget at all, it is unlisted, and with grade-retry-flakes.sh gating `report` an unlisted flake fails the run.

**Where.** crates/wcore-agent/tests/spill_readback_engine_wiring.rs::the_engine_spills_where_this_session_can_read_it_back ; budgets at .config/nextest.toml:72 (default) and :278 (ci)

**Why it matters.** Either the spill/read-back path really can take >60s under contention, which is a product latency finding on a path users hit, or the test's fixture is doing something disproportionate. Nobody has graded which. It surfaced only because I ran the suite at --retries 0; under CI's retries=2 it would come back as FLAKY-then-pass and, being unlisted, would redden the required `report` context with no diagnostic attached -- the exact failure shape #337 itself was filed about.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
