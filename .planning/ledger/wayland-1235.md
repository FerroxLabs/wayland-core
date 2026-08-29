---
issue: 1235
repo: FerroxLabs/wayland
kind: defect
title: "One mock turn with a 480 KB tool result costs 63.6 s of CPU; the spill/read-back test is killed by the default nextest budget 3/3"
status: open
last_verified_commit: c12c8033
criteria:
  - id: c1
    text: "Where the 63.6 s of CPU goes is measured and named, not guessed"
    state: not-met
    owner: core
  - id: c2
    text: "The cost on a 480 KB shed is either brought down or documented as justified on the turn loop a user hits"
    state: not-met
    owner: core
  - id: c3
    text: "The fixture constant is tied to the thresholds it must exceed, so the test cannot stop spilling and still pass"
    state: not-met
    owner: core
    note: "Measured: at `\"x\".repeat(120_000)` the test FAILS in 14.64 s user CPU while 60_000 passes in 56.15 s and 240_000 passes in 32.60 s. The property under test (a spill happened) disappears when one constant is halved, and that constant is unrelated in the source to max_result_size = 600_000, context_window = 60_000, output_reserve = 10_000 and emergency_buffer = 10_000."
  - id: c4
    text: "If a nextest budget override is still needed after c1-c3, it carries the measured number rather than a round one"
    state: not-met
    owner: core
    note: "DELIBERATELY LAST. An allowlist line or a raised budget added before c1 would suppress the symptom of a product cost nobody has measured, which is why this was filed instead of allowlisted."
---

Found while verifying wayland-core#337 — a different test in the same crate —
and filed separately because it is a different defect.

`wcore-agent::spill_readback_engine_wiring::the_engine_spills_where_this_session_can_read_it_back`
drives ONE mock turn: a `MockLlmProvider` with two scripted turns and a
`MockTool` returning `"x".repeat(480_000)`. No provider, no network, no sleep.
Running the compiled binary directly on hetzner-dsm under `/usr/bin/time -v`:
76.26 s wall, **63.60 s user**, 0.10 s system, 38 MB peak RSS. It is not blocked
on the disk and it is not swapping; it is a minute of single-threaded compute on
one 480 KB tool result, on the turn loop a real user hits.

Under `[profile.default]` (`slow-timeout = 30s, terminate-after = 2`) it is
KILLED — 3 of 3 runs alone at `--retries 0`, `TIMEOUT [ 60.011s]`. `[profile.ci]`
gives 90 s x 2 so CI probably does not kill it, but it is the one test in this
crate close enough to a kill budget to cross it, it is not in
`.config/flaky-allowlist.txt`, and under CI's `retries = 2` a kill returns as
FLAKY-then-pass, which `grade-retry-flakes.sh` reds the required `report` check
for with no diagnostic attached.

Not allowlisted on purpose. See c4.
