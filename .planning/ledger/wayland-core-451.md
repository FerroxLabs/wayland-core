---
issue: 451
repo: FerroxLabs/wayland-core
kind: defect
title: "Four of five mutation legs cannot run their unmutated test suite inside the per-crate timeout, so they test zero mutants"
status: open
last_verified_commit: 57e2a244e
criteria:
  - id: c1
    text: "Every mutation leg either produces a `N mutants tested in` summary line in its CI artifact, or is explicitly and durably declared out of scope with a recorded reason -- no leg silently produces nothing."
    state: not-met
    evidence: ""
    owner: core
    note: "MEASURED on run 33844721279 (event=schedule, head_sha=57e2a244e, all legs hosted macos-latest, run conclusion FAILURE). Four legs concluded failure at step 8 `Run cargo-mutants` -- wcore-config, wcore-providers, wcore-cli, wcore-agent -- each ending `ERROR cargo test failed in an unmutated tree, so no mutants were tested`. Step conclusions read from the structured jobs API, NOT grepped from log text; core#424's own history records that grepping the job log returned 10 false hits for the summary line where the truth was 0, because a job log echoes the workflow's own YAML comments. The single passing leg wcore-cron reported `339 mutants tested in 52m: 64 missed, 196 caught, 77 unviable, 2 timeouts`, read from artifact mutants-log-wcore-cron. So mutation coverage outside wcore-cron is ZERO. This is NOT a regression: until core#424 landed, the leg ended `exit 0   # Never fail the matrix leg` and 87 consecutive runs were green while producing nothing. Arming the gate is what made this visible."
  - id: c2
    text: "The per-crate timeouts are set from a MEASURED unmutated-tree build+test time for that crate on the hosted runner, with the measurement recorded in the change that sets them -- not guessed."
    state: not-met
    evidence: ""
    owner: core
    note: "Written to forbid the tempting fix. The observed unmutated-tree costs on the hosted runner are wcore-config 307s build + 180s test, wcore-providers 457s + 60s, wcore-cli 1009s + 120s, and wcore-agent timing out on the test list alone. wcore-cli therefore needs more than 1129s for the unmutated tree BEFORE a single mutant runs, so a guessed bump that still falls short buys another silent night and looks identical to a fix. Record the measurement beside the number so the next person can tell a derived timeout from a hopeful one. Worth asking whether a debug-profile unmutated baseline is even the right unit here, or whether these legs need different granularity (per-module, or a build cache shared across the matrix) -- raising four timeouts may be treating the symptom."
  - id: c3
    text: "A run in which a leg produces no mutation data still concludes `failure`, AND a leg that produces a finding still concludes `success` -- both directions, shown on the same run where possible."
    state: met
    evidence: "file:.github/workflows/mutants-nightly.yml:214:This is a harness failure, not a coverage result."
    owner: core
    note: "MET ALREADY, and recorded here so a fix for c1/c2 cannot quietly destroy it. Both directions are proven by run 33844721279 in a single run: the four data-less legs concluded FAILURE, while wcore-cron found 64 SURVIVING mutants (cargo-mutants exit 3) and still concluded SUCCESS, filing core#449. That asymmetry is the whole point -- the gate branches on REAL_DATA rather than on the exit code, so a harness failure reds and a genuine finding does not. The forbidden repairs for c1/c2 are exactly the two that would break this: lowering --timeout back into silence, or restoring an unconditional `exit 0`. Either one reopens core#424. A gate that cannot fail is worth as little as one that cannot pass."
---

# Mutation coverage is 1 of 5 crates, and the nightly now reds every night

Filed after `core#424` armed the harness gate. `core#424` fixed the reporting defect
-- the leg can now tell you when it produced nothing -- and this issue is what that
honesty immediately revealed.

Read the two issues together: `core#424`'s complaint was "zero data across 87 runs".
The harness is repaired; the coverage is not. The honest status is 1 of 5 crates.

**Do not fix this by disarming the gate.** The nightly reding every night is the
correct behaviour for a tree where four legs test nothing. The red should end because
the legs start producing data, not because they stop being asked.
