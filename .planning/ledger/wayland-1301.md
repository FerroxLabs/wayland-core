---
issue: 1301
repo: FerroxLabs/wayland
kind: defect
title: "First Windows retry-flake cluster: a wall-clock ratio guard and a dispatch-budget test at 94% of its kill line (CI (Array))"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "one_turn_costs_about_two_whole_payload_scrub_passes has its estimator changed to reduce variance WITHOUT moving either bound, and is re-measured at n>=40 under load with a stated failure rate. Widening the 1.5..2.5 window does not satisfy this."
    state: not-met
    owner: core
    note: "Filed 2026-09-03. MEASURED, and reproduced off Windows so it is not a platform artifact: Windows 1/10 (run 33707672948, 'one turn now costs 3.06 whole-payload PIIScrubber passes per byte ... small=0.7434s large=1.4755s one_scrub=0.2744s'), and Linux 1/41 at --retries 0 under load 62-151 with payload 'ratios=[1.46, 2.68, 4.61] median_passes=2.68' -- ONE invocation crossing BOTH bounds on a byte-identical tree. Over 120 rounds, 7 were >= 2.5 (max 4.61) and 3 were <= 1.5; only the median-of-3 hides them. NOT A REGRESSION: over 41 Linux runs the asserted median-of-3 is min 1.81 / median 2.00 / max 2.68, exactly the 2.0 that wayland-core#395 measured. The estimator is a DIFFERENCE of two wall-clock samples divided by a third, so noise is amplified and ~290ms of jitter in the large arm moves it a full pass. The criterion forbids widening the bounds on purpose: the LOWER bound is the anti-'stop scrubbing' control and loosening it would silently retire the property the test exists for."
  - id: c2
    text: "The justifying comment in .config/nextest.toml no longer claims CI's 180s budget covers fix1_dispatch_budget_aborts_with_partial_result; it states the real 240s inherited override and the measured 185-226s range."
    state: not-met
    owner: core
    note: "The comment certified a budget the test has NEVER ONCE passed inside. All ten measured Windows runs exceed 180s (185.46-226.28s); it passes only because nextest inherits [[profile.default.overrides]] into --profile ci, giving 120s x 2 = 240s. A false justification in a config file is worse than none: it tells the next reader the question is settled."
  - id: c3
    text: "fix1_dispatch_budget_aborts_with_partial_result has at least 1.5x headroom against its kill line on Windows, measured at n>=10. Today it sits at 90-94%."
    state: not-met
    owner: core
    note: "DRIFTING, which is why this is a criterion and not an observation: 185.5/188.0/194.2s on 2026-09-02 against 216.9/221.0/226.3s on 2026-09-03. The failure that surfaced was <flakyFailure type='test timeout' time='240.072'> with EMPTY payload -- no assertion ran, so this is a budget failure and not the dispatch-bound correctness property. Negative controls, genuinely executing: Linux 0/12 at --retries 0 (83.70-99.33s, 2.4x headroom) and macOS 0/10 (54.76-96.23s). The cost is INHERENT -- ~47ms of inline CPU per dispatch in a debug build, already recorded in that same comment block -- so a retry cannot help and raising retries would only hide it."
  - id: c4
    text: "Both allowlist entries added for these two tests are DELETED, not renewed, when c1 and c3 land."
    state: not-met
    owner: core
    note: "The allowlist header states its own rule: the list is designed to shrink, and an entry means someone owns the debt with a date on it. Recording the deletion as a criterion is what stops the entries being renewed by whoever hits the expiry."
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
