---
issue: 337
repo: FerroxLabs/wayland-core
kind: defect
title: "Fragile 4s wall-clock bound: dangerous_expiry_cancels_production_streaming_bash_process_tree"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The wall-clock assertion bounds cancellation only, not bootstrap plus pid publication plus cancellation"
    state: met
    evidence: "test:crates/wcore-agent/tests/dangerous_lease_e2e_test.rs::dangerous_expiry_cancels_production_streaming_bash_process_tree"
    owner: core
    note: "The 4s assertion is DELETED (dc7aa695), not widened. One clock: granted = Instant::now() before bootstrap, and TREE_UP_BUDGET / CANCELLATION_BUDGET / REAP_BUDGET are absolute offsets from it. The only remaining wall clock is abort_by = granted + LEASE_TTL + CANCELLATION_BUDGET, stated against EXPIRY rather than fixture setup. LEASE_TTL raised 3s to 6s so the phases sum inside the bound."
  - id: c2
    text: "A mutation that stops lease expiry cancelling the process tree still turns the test red"
    state: not-met
    owner: core
    note: "Structurally all three coverage points survive the deletion - the timeout on the run future, matches!(outcome, Err(AgentError::UserAborted)), and wait_gone on both shell and child pid - so an expiry that stopped cancelling would hang past abort_by. But that outcome depends on real process and timer behaviour, not on one readable assertion. MUTATION ARM NOT RUN. The structural argument is recorded above, but this criterion asserts an OBSERVED outcome and nothing in the tree records one. The standing rule in this repo is that a test nobody watched fail is not evidence, so it grades not-met until one cheap run flips it."
  - id: c3
    text: "Artificially delaying pid publication no longer turns the test red"
    state: not-met
    owner: core
    note: "read_pid now takes an ABSOLUTE deadline (granted + TREE_UP_BUDGET = 4s) instead of two independent 2s budgets, which is the right shape. RESIDUAL: bootstrap time is charged against the same 4s, so the headroom for an artificial pid-publication delay is 4s minus bootstrap, not a full 4s, and that headroom has not been measured. MUTATION ARM NOT RUN. The structural argument is recorded above, but this criterion asserts an OBSERVED outcome and nothing in the tree records one. The standing rule in this repo is that a test nobody watched fail is not evidence, so it grades not-met until one cheap run flips it."
  - id: c4
    text: "The flaky-allowlist entry for this test is deleted rather than renewed"
    state: met
    evidence: "commit:c461293fdd39849da0d0b93c224d7219ba5b334d"
    owner: core
    note: "Deleted, not renewed. git show c461293f removes the line for this test and for its sibling dangerous_expiry_reaches_bootstrapped_spawn_child, whose test was re-stated on the lease clock in the same commit. Absence verified with a known-positive control in the same command: grep for this test name in .config/flaky-allowlist.txt returns nothing while grep -c crucible_council returns 1."
---

The issue asks whether the four-second wall-clock bound in this end-to-end lease
test is a real product requirement or an arbitrary constant. Reading the test
answers it, and the answer is neither.

The clock is started before the run is spawned. Two pid reads with two-second
timeouts apiece sit inside that window, and the cancellation being measured has
its own four-second timeout after them. So every individual budget can be
honoured and the final assertion still fails. The 6.4 seconds observed in CI is
inside the permitted envelope. Its sibling test carries no such redundant
assertion; only this one does.

There is a second-order problem worth naming: the allowlist entry that masks
this test cites a different failure mechanism entirely - the pid-read timeout,
not the four-second bound - so the mask and the ticket are not talking about the
same thing. The fix here is a deletion or a re-baseline, one file, fully
verifiable on Linux.

Criteria come from the cluster C verification note of 2026-08-29.
