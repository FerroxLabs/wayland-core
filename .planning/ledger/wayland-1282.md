---
issue: 1282
repo: FerroxLabs/wayland
kind: defect
title: "dangerous_expiry_cancels_production_streaming_bash_process_tree fails only under full-suite contention, and is not allowlisted"
status: open
last_verified_commit: 88a87e5ee
criteria:
  - id: c1
    text: "The failure rate is MEASURED where it actually occurs -- at --retries 0 under a concurrent full-suite load, n at least 20 -- rather than inferred from isolated runs"
    state: not-met
    evidence: "test:crates/wcore-agent/tests/dangerous_lease_e2e_test.rs::dangerous_expiry_cancels_production_streaming_bash_process_tree"
    owner: core
    note: "STILL NOT MET. The condition this criterion names -- --retries 0 under a CONCURRENT FULL-SUITE LOAD, n>=20 -- was not produced, and nothing here is offered in its place. This lane holds one build slot on hetzner-dsm behind a flock; running a full workspace suite alongside the sample would take the host from other lanes and would still not be the containerised leg the failure was observed on. WHAT WAS ACHIEVED, labelled as the WRONG condition so it cannot be misread as the measurement: 1/1 pass at 88a87e5ee, `cargo test -p wcore-agent --test dangerous_lease_e2e_test dangerous_expiry_cancels_production_streaming_bash_process_tree`, 6.20s in-test, hetzner-dsm 1-min loadavg ~28. That is an isolated pass at moderate ambient load and it adds nothing the ledger's existing 6/6 did not already say. WHAT THE PAYLOAD SAYS THE FAILURE IS, and it is NOT teardown: the allowlist entry for this test quotes `dangerous_lease_e2e_test.rs:110:6 ... the Dangerous Bash process tree must be up before the lease expires ... Elapsed(())`, i.e. `read_pid` timing out against TREE_UP_BUDGET (4 s) inside LEASE_TTL (6 s). Bootstrap plus session init plus shell spawn has to fit in 4 s, and under contention it does not -- so the containment property never runs and a SETUP loss is reported as a containment red. WHAT IS OWED: n>=20 at --retries 0 on the containerised leg with a concurrent full-suite load, its run identity and load recorded, before either branch of c2 can be taken."
  - id: c2
    text: "Either the teardown timing is fixed, or .config/flaky-allowlist.txt carries an entry stating the measured rate and the load condition it was measured under -- never an entry resting on isolated passes"
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "STILL NOT MET, on both branches, and the file was deliberately left untouched. BRANCH TWO IS ALREADY REFUTED BY WHAT IS IN THE FILE: .config/flaky-allowlist.txt carries a gh#1282 entry for this test whose own text ends `RATE NOT MEASURED. REMOVE when gh#1282 lands; do NOT renew` -- an entry resting on a single observation is exactly the shape this criterion forbids, so renewing or re-wording it cannot close this. Adding a second bare entry would be worse. BRANCH ONE (fix the timing) WAS SCOPED AND NOT ATTEMPTED, stated rather than quietly skipped: the construction is already one-clock and already correct in shape (core#337 removed the four contradictory budgets); what remains is that TREE_UP_BUDGET races real setup inside a REAL PRODUCT LEASE, so the honest fix is a test-only seam that arms lease expiry on an EVENT -- both PIDs published and a streaming chunk observed -- instead of on wall time. That seam reaches into the dangerous-grant expiry path in product code, and it cannot be validated by this lane because the only condition that falsifies it is the loaded one c1 owes. Building it blind and grading it at 1/1 on a quiet host would be the false close this ticket exists to prevent. WHAT IS OWED: c1's loaded n>=20 first, then either the event-armed lease seam or an allowlist entry stating that measured rate AND its load."
---

# A containment test that intermittently cannot prove containment

Not a regression: an interleaved A/B at `--retries 0` puts the post-landing tree and
`ca15a48bf` at 6 pass / 0 fail each. It fails only under full-suite parallelism, which makes
it a contention-dependent teardown timeout rather than a logic defect -- and it is not in the
flaky allowlist, so today it is a red nobody owns.
