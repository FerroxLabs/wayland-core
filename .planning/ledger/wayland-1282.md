---
issue: 1282
repo: FerroxLabs/wayland
kind: defect
title: "dangerous_expiry_cancels_production_streaming_bash_process_tree fails only under full-suite contention, and is not allowlisted"
status: open
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "The failure rate is MEASURED where it actually occurs -- at --retries 0 under a concurrent full-suite load, n at least 20 -- rather than inferred from isolated runs"
    state: not-met
    evidence: "test:crates/wcore-agent/tests/dangerous_lease_e2e_test.rs::dangerous_expiry_cancels_production_streaming_bash_process_tree"
    owner: core
    note: "Filed 2026-08-31 by the core lane during the f13 landing pass. NOT a release blocker and not a regression, both established by measurement rather than argument. The landing merged changes to process_tree.rs, which is this test own territory, so my merges did not touch the test file was not a good enough reading. A/B at --retries 0, six trials per arm, arms INTERLEAVED so a load spike hits both equally: head (integ/f13 after the landing) 6 pass / 0 fail; base (ca15a48bf, before it) 6 pass / 0 fail; host load 27-37 recorded per trial. The arms are indistinguishable, so nothing in the landing caused this. WHAT IS OWED: it reproduces ONLY under full-suite parallelism -- one failure alongside 17,780 other tests, zero in six isolated trials -- which is a contention-dependent teardown timeout, not a logic defect. It must NOT be allowlisted on those six passes: that file discipline is that an entry states what it measured, and six isolated passes measure the wrong condition. Measure at --retries 0 under a concurrent full-suite load, n at least 20, then fix the teardown or write an entry stating the rate AND the load it was measured under. Why it outranks an ordinary flake: the test asserts that an expired dangerous lease CANCELS a production streaming bash process tree, so a test that intermittently cannot prove containment is one nobody can read a containment claim off."
  - id: c2
    text: "Either the teardown timing is fixed, or .config/flaky-allowlist.txt carries an entry stating the measured rate and the load condition it was measured under -- never an entry resting on isolated passes"
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "Six isolated trials at --retries 0 could not provoke this while one full-suite run did, so an allowlist entry written from isolated runs would record a rate the test does not have under the condition that actually fails it. That is the exact shape this file forbids."
---

# A containment test that intermittently cannot prove containment

Not a regression: an interleaved A/B at `--retries 0` puts the post-landing tree and
`ca15a48bf` at 6 pass / 0 fail each. It fails only under full-suite parallelism, which makes
it a contention-dependent teardown timeout rather than a logic defect -- and it is not in the
flaky allowlist, so today it is a red nobody owns.
