---
issue: 1238
repo: FerroxLabs/wayland
kind: defect
title: "Flaky: parallel_spawn_caps_active_child_engines_across_shared_calls fails on a 15s wall-clock budget above loadavg ~190, not on its invariant"
status: open
last_verified_commit: 509f4426b
criteria:
  - id: c1
    text: "— The test's second phase does not fail on a wall-clock budget that host load can exhaust. Either the 15s `tokio::time::timeout` is replaced by a condition the test can wait on unboundedly (with the harness-level timeout as the only backstop), or the budget is derived from something measured rather than a literal, with the derivation stated in the source."
    state: met
    evidence: "symbol:crates/wcore-agent/src/spawner.rs::DRAIN_BACKSTOP"
    owner: core
    note: "MET at 509f4426b, by the SECOND arm this criterion offers. The literal 15s `tokio::time::timeout` is gone; the drain is now bounded by `const DRAIN_BACKSTOP: Duration = Duration::from_secs(120)` (spawner.rs:3421) and the derivation is stated in the source immediately above it: 120s is 4.4x the worst failure ever measured for this test (26.997s at loadavg ~211) and ~8x the old literal, chosen so the bound can only fire on a genuine hang while still bounding a leg where `cargo test` applies no per-test timeout of its own. The FIRST arm -- wait unboundedly with the harness as the only backstop -- was rejected in the same change, and the reason is recorded there: the leg that reddens is `Shared-process lib suite (cargo test, one process per binary)`, where an unbounded wait would let a real hang run to the job 150-minute wall. Landed in 774c40f5a. WHAT WOULD FALSIFY THIS: the const being replaced by a bare literal again, or the derivation comment being deleted -- the symbol token reds on the first."
  - id: c2
    text: "— The failure the test CAN still produce names the invariant, not the clock: with the concurrency cap deliberately broken, the panic message says the cap was exceeded and quotes the observed `peak`."
    state: met
    evidence: "commit:774c40f5a"
    owner: core
    note: "MET at 509f4426b. Both `peak` assertions now name the invariant: spawner.rs:3389 and :3448 each read `the shared active-child cap was exceeded: peak={} against a limit of {}` and interpolate the observed `provider.peak`, and the backstop path panics with `queued children did not all run within {DRAIN_BACKSTOP:?} of permits being released: active={} peak={} calls={} of {TOTAL_CHILDREN} expected, cap={}` rather than with a bare timeout. Anchored on the commit rather than on a `file:` fragment because the message is DUPLICATED across the two assert sites, and a fragment matching twice pins nothing under this gate rule. The landing commit records the forced-path verification verbatim (backstop mutated to 1ms, restored after): `queued children did not all run within 120s of permits being released: active=0 peak=20 calls=21 of 100 expected, cap=20`. NOT GRADED HERE: c3 and c4, which need a re-measurement at loadavg above 190 and a TRY-n-FAIL census across CI logs; neither exists in the tree."
  - id: c3
    text: "— Re-measured at retries=0 over N ≥ 20 **at a load average above 190**, with a known-positive control (cap deliberately broken) in the same run, both binaries identified by sha256. A measurement taken below that load does not close this — the table above shows it passes 36/36 there."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
  - id: c4
    text: "— `TRY n FAIL` occurrences for this test are counted across recent CI logs, not just run conclusions, so the retry-masked rate is on the record before and after."
    state: not-met
    owner: core
    note: "Transcribed from the issue body verbatim on 2026-08-31. This ledger did not exist until now: the issue was filed 2026-08-29/30 by this cycle's own verification and never entered the release gate, which counts only issues holding a ledger file. State is not-met because no lane has claimed it and nothing in the tree has been graded against this text. kind is defect, not task, because the gate reserves task for a credential, an account or a platform a human must obtain and there is code behind this one."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
