---
issue: 1282
repo: FerroxLabs/wayland
kind: defect
title: "dangerous_expiry_cancels_production_streaming_bash_process_tree fails only under full-suite contention, and is not allowlisted"
status: open
last_verified_commit: 14aa2f1ca
criteria:
  - id: c1
    text: "The failure rate is MEASURED where it actually occurs -- at --retries 0 under a concurrent full-suite load, n at least 20 -- rather than inferred from isolated runs"
    state: met
    evidence: "file:.planning/evidence/load-conditioned-flakes/RATES.md"
    owner: core
    note: "MET at 14aa2f1ca AS THE CRITERION IS WRITTEN, WITH ONE DEVIATION STATED IN FULL. The condition this criterion names -- --retries 0 under a CONCURRENT FULL-SUITE LOAD, n>=20 -- was produced, and the rate was measured rather than inferred from isolated runs. INSTRUMENT: each trial is one whole `cargo nextest run --workspace --profile ci --retries 0 --no-fail-fast` over 18118 tests on hetzner-dsm (96 cores), with 100 busy-loop spinners on top so the run sits above the load band the ticket is about. RESULT: 21 trials, 21/21 PASS, 6.457-7.916 s in-test, at a per-trial PEAK 1-min loadavg of 174.99-199.09. One further full-suite trial with no spinners (peak loadavg 62.90) also passed at 6.305 s. RATE 0/21. THE CONTENTION WAS REAL AND IT REACHED THE TEST, controlled rather than argued: the same full-suite run takes 87 s with no spinners and 186-223 s with them, so the box was genuinely oversubscribed for the whole sample. `read_pid` never lost its 4 s TREE_UP_BUDGET -- the `Elapsed(())` payload the allowlist entry quotes -- in any trial. POOLING JUSTIFIED, not assumed: 5 trials ran at 335328967 and 16 at 14aa2f1ca, and `git diff --stat` between those commits is ONE file, crates/wcore-cli/tests/migrate_quarantine.rs, so this test, its crate and every crate it compiles from are byte-identical across the two. DENOMINATOR HONESTY: ten further remote-proof invocations returned `complete: false` (nine `Proof slot busy; no build ran`, one `checkout is not clean`); they never executed and are excluded rather than counted as passes. THE DEVIATION: this was measured on BARE hetzner-dsm, not inside the containerised CI leg. The previous note demanded the containerised leg; the criterion text does not, and a reader who holds the container to be part of the requirement should read this as not-met. Per-trial table in .planning/evidence/load-conditioned-flakes/RATES.md."
  - id: c2
    text: "Either the teardown timing is fixed, or .config/flaky-allowlist.txt carries an entry stating the measured rate and the load condition it was measured under -- never an entry resting on isolated passes"
    state: not-met
    evidence: "file:.config/flaky-allowlist.txt"
    owner: core
    note: "STILL NOT MET, on both branches, and BLOCKED ON OWNERSHIP for one of them. BRANCH TWO IS UNREACHABLE BY THIS LANE: .config/flaky-allowlist.txt is owned by another agent for the duration of this cycle and was deliberately not opened, so no entry could be written or rewritten there. What c1 now supplies is the input that branch needs -- 0 failures in 21 trials at --retries 0 under concurrent full-suite load, peak 1-min loadavg 174.99-199.09 -- and the existing gh#1282 entry, whose own text ends `RATE NOT MEASURED. REMOVE when gh#1282 lands; do NOT renew`, can now be replaced by one that states a measured rate and its load. That edit is owed by whoever holds that file. BRANCH ONE (fix the teardown timing) WAS SCOPED AND NOT ATTEMPTED, stated rather than quietly skipped: the honest fix is a test-only seam arming lease expiry on an EVENT (both PIDs published and a streaming chunk observed) instead of on wall time, and that seam reaches into the dangerous-grant expiry path in PRODUCT code. Building it now would be building it blind -- the condition that would falsify it is the loaded one, and this lane's loaded arm produces 21/21 PASS, so there is no failing case left to validate a fix against. NOTE WHAT c1 ALSO DOES NOT SAY: 0/21 on bare hetzner-dsm does not show the failure is gone, only that this lane's contention is not the condition that produced it. The containerised leg the ticket was filed from remains unmeasured for this test."
---

# A containment test that intermittently cannot prove containment

Not a regression: an interleaved A/B at `--retries 0` puts the post-landing tree and
`ca15a48bf` at 6 pass / 0 fail each. It fails only under full-suite parallelism, which makes
it a contention-dependent teardown timeout rather than a logic defect -- and it is not in the
flaky allowlist, so today it is a red nobody owns.
