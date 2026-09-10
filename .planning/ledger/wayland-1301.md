---
issue: 1301
repo: FerroxLabs/wayland
kind: defect
title: "First Windows retry-flake cluster: a wall-clock ratio guard and a dispatch-budget test at 94% of its kill line (CI (Array))"
status: open
last_verified_commit: 6342f1b90
criteria:
  - id: c1
    text: "one_turn_costs_about_two_whole_payload_scrub_passes has its estimator changed to reduce variance WITHOUT moving either bound, and is re-measured at n>=40 under load with a stated failure rate. Widening the 1.5..2.5 window does not satisfy this."
    state: met
    evidence: "file:crates/wcore-agent/tests/turn_cost_per_byte_guard.rs:200:MEASURED 2026-09-10 (wayland#1301 c1) on SeanDesktop"
    owner: core
    note: "MET at 29cbff29. The estimator is now a median of SEVEN interleaved rounds (`const ROUNDS: usize = 7` at :218, `ratios[ratios.len() / 2]` at :232); both bounds are byte-identical at 1.5 and 2.5, and every raw ratio is still printed. RE-MEASURED on SeanDesktop -- the box that hosts the `CI (Array)` runner services -- 32 cores with 16 CPU spinners pinned against them, `--retries 0`, n=40 per arm, the old 3-round estimator at fdf4b1e1c against the new one at 785a22ea. STATED FAILURE RATE: 0/40 at BOTH arms. That population did NOT reproduce the 1/10 seen on CI (Array) or the 1/41 seen on Linux, so this run cannot and does not claim a rate improvement it never observed -- what it measures is DISTANCE to failure on the same box under the same load. Asserted median across the 40 runs: 1.690-2.020 sd 0.070 at 3 rounds against 1.820-2.130 sd 0.058 at 7, worst approach to a bound +0.190 against +0.320. Resampling every 3-subset of each 7-round run (which holds the load epoch fixed, where drawing rounds independently would not) a median of 3 came within +0.030 of a bound over 1400 subsets while the median of 7 of those same rounds stayed +0.320 away. The individual rounds are as noisy as ever: 34 of 280 fell outside 1.5..2.5 on their own, min 1.20 max 2.69, which is the point -- the estimator, not the product, was the flake. LIMIT: the box was otherwise idle, its three runner services were not live, and n=40 is far too small to bound a ~2 percent event. ORIGINAL FILING, kept: Windows 1/10 (run 33707672948, 'one turn now costs 3.06 whole-payload PIIScrubber passes per byte ... small=0.7434s large=1.4755s one_scrub=0.2744s'), and Linux 1/41 at --retries 0 under load 62-151, payload 'ratios=[1.46, 2.68, 4.61] median_passes=2.68' -- ONE invocation crossing BOTH bounds on a byte-identical tree. NOT A REGRESSION: over 41 Linux runs the asserted median-of-3 is min 1.81 / median 2.00 / max 2.68, exactly the 2.0 that wayland-core#395 measured. so it is not a platform artifact: Windows 1/10 (run 33707672948, 'one turn now costs 3.06 whole-payload PIIScrubber passes per byte ... small=0.7434s large=1.4755s one_scrub=0.2744s'), and Linux 1/41 at --retries 0 under load 62-151 with payload 'ratios=[1.46, 2.68, 4.61] median_passes=2.68' -- ONE invocation crossing BOTH bounds on a byte-identical tree. Over 120 rounds, 7 were >= 2.5 (max 4.61) and 3 were <= 1.5; only the median-of-3 hides them. NOT A REGRESSION: over 41 Linux runs the asserted median-of-3 is min 1.81 / median 2.00 / max 2.68, exactly the 2.0 that wayland-core#395 measured. The estimator is a DIFFERENCE of two wall-clock samples divided by a third, so noise is amplified and ~290ms of jitter in the large arm moves it a full pass."
  - id: c2
    text: "The justifying comment in .config/nextest.toml no longer claims CI's 180s budget covers fix1_dispatch_budget_aborts_with_partial_result; it states the real 240s inherited override and the measured 185-226s range."
    state: met
    evidence: "file:.config/nextest.toml:139:CORRECTED 2026-09-03 (gh#1301 c2)"
    owner: core
    note: "MET at 509f4426b. The justifying comment in .config/nextest.toml no longer certifies the 180s budget, and it states both figures this criterion names. Read in the tree at :139-149: it records that the block used to read `this test already PASSES in CI, whose 90s x 2 = 180s budget covers it` and that this was FALSE -- all ten measured Windows (`CI (Array)`) runs exceed 180s, at 185.46-226.28s, so the test has never once passed inside the budget the comment certified. It names the real inherited override (`nextest inherits THIS override into --profile ci, giving it 120s x 2 = 240s`, matching `slow-timeout = { period = 120s, terminate-after = 2 }` at :162) and the measured range and its drift (185-194s on 2026-09-02 against 216.9/221.0/226.3s on 2026-09-03, i.e. 90-94 percent of the kill line). Landed in 774c40f5a. c3 is NOT graded by this: the headroom is still 90-94 percent, not the 1.5x the criterion requires."
  - id: c3
    text: "fix1_dispatch_budget_aborts_with_partial_result has at least 1.5x headroom against its kill line on Windows, measured at n>=10. Today it sits at 90-94%."
    state: not-met
    owner: core
    note: "STILL NOT MET at 29cbff29, and deliberately NOT closed on the arm that would have allowed it. RE-MEASURED on SeanDesktop -- the box that hosts the `CI (Array)` runner services -- n=10 per arm at --retries 0, in-test seconds against the 240s kill (`.config/nextest.toml` :161-162, 120s x terminate-after 2, inherited by --profile ci): IDLE BOX 148.75-154.25s avg 152.87, headroom at max 1.56x. LOADED (16 CPU spinners against 32 cores) 185.49-187.32s avg 186.72, headroom at max 1.28x. The idle arm alone reads as a pass against the literal text of this criterion and it is NOT accepted as one: nothing in this branch touches this test or its cost, the improvement over the recorded 216-226s is entirely the absence of the concurrent runner services, and the criterion exists because the CI condition is contended. Ordered by contention the same host gives 1.56x idle, 1.28x under a spinner proxy, and 1.06-1.11x under real CI concurrency -- so the headroom crosses 1.5x only when nothing else is running. WHAT IS OWED: either a measured >=1.5x under real CI (Array) concurrency, or a cost reduction in the fixture that preserves the genuine MAX_TOTAL_DISPATCHES crossing. NOT AVAILABLE HERE: the abort fires at ~1000 dispatches regardless of the 1200 requested, so shrinking the `over:` array saves nothing, and the per-dispatch cost is inline engine CPU (~47ms debug) inside `AgentSpawner`, outside this lane's ownership. Raising terminate-after 2 -> 3 would satisfy the arithmetic and is explicitly excluded: this criterion is not closable by moving the kill line. HISTORY, kept -- DRIFTING, which is why this is a criterion and not an observation: 185.5/188.0/194.2s on 2026-09-02 against 216.9/221.0/226.3s on 2026-09-03. The failure that surfaced was <flakyFailure type='test timeout' time='240.072'> with EMPTY payload -- no assertion ran, so this is a budget failure and not the dispatch-bound correctness property. Negative controls, genuinely executing: Linux 0/12 at --retries 0 (83.70-99.33s, 2.4x headroom) and macOS 0/10 (54.76-96.23s). The cost is INHERENT -- ~47ms of inline CPU per dispatch in a debug build, already recorded in that same comment block -- so a retry cannot help and raising retries would only hide it."
  - id: c4
    text: "Both allowlist entries added for these two tests are DELETED, not renewed, when c1 and c3 land."
    state: not-met
    owner: core
    note: "STILL NOT MET at 29cbff29, and it cannot be met yet: this criterion is conditioned on c1 AND c3 landing, and c3 is open. c1 has landed, so `.config/flaky-allowlist.txt` :74 (one_turn_costs_about_two_whole_payload_scrub_passes) is now discharged and should be DELETED -- its own text already names the fix that shipped, `median-of-3 -> median-of-7`. Line :75 (fix1_dispatch_budget_aborts_with_partial_result) must STAY until c3 is genuinely met; deleting it on the idle-box 1.56x would re-red CI at the first contended run. Neither edit is made here: a concurrent worker holds pending deletions in that same file, so both are reported to the release owner. Flip to met with `absent:.config/flaky-allowlist.txt::one_turn_costs_about_two_whole_payload_scrub_passes` once BOTH are gone. The allowlist header states its own rule: the list is designed to shrink, and an entry means someone owns the debt with a date on it. Recording the deletion as a criterion is what stops the entries being renewed by whoever hits the expiry."
---

# Windows had no flake-cluster ticket. These are the first two.

macOS has #1286 and Linux has #1288. This is the Windows equivalent, and the two
members are different diseases sharing one condition: a self-hosted 32-core box
running up to three runner services at once.

One is a wall-clock ratio guard whose estimator amplifies scheduler noise; the other
is a test that has quietly grown to 94% of its kill line while a comment in the repo
certified that a budget it has never passed inside covers it.

METHOD NOTE WORTH KEEPING. All three of that run's flakes were first attributed to
`linux-containerized`. Grading the raw `<flakyFailure>` entries per leg gave
Array 3, linux-containerized 1 (an already-allowlisted `f14_*`), macos-latest 0.
Grade every leg's artifact, not the one you expect to be guilty.
