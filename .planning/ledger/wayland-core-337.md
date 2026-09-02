---
issue: 337
repo: FerroxLabs/wayland-core
kind: defect
title: "Fragile 4s wall-clock bound: dangerous_expiry_cancels_production_streaming_bash_process_tree"
status: closed
last_verified_commit: 93ede3424
criteria:
  - id: c1
    text: "The wall-clock assertion bounds cancellation only, not bootstrap plus pid publication plus cancellation"
    state: met
    evidence: "test:crates/wcore-agent/tests/dangerous_lease_e2e_test.rs::dangerous_expiry_cancels_production_streaming_bash_process_tree"
    owner: core
    note: "The 4s assertion is DELETED (dc7aa695), not widened. One clock: granted = Instant::now() before bootstrap, and TREE_UP_BUDGET / CANCELLATION_BUDGET / REAP_BUDGET are absolute offsets from it. The only remaining wall clock is abort_by = granted + LEASE_TTL + CANCELLATION_BUDGET, stated against EXPIRY rather than fixture setup. LEASE_TTL raised 3s to 6s so the phases sum inside the bound."
  - id: c2
    text: "A mutation that stops lease expiry cancelling the process tree still turns the test red"
    state: met
    evidence: "test:crates/wcore-agent/tests/dangerous_lease_e2e_test.rs::dangerous_expiry_cancels_production_streaming_bash_process_tree"
    owner: core
    note: "MUTATION ARM RUN, hetzner-dsm 2026-08-29. In crates/wcore-agent/src/cancel.rs::arm_dangerous_lease the expiry task's two cancelling statements (root.cancel(); handle.active_turn_token().cancel();) were deleted while termination.mark(SessionTerminationReason::DangerousLeaseExpired) was LEFT IN PLACE -- so the timer still fires and the task still runs, and the only thing withdrawn is the cancellation. The diff was read back: the mutation landed on those two CODE statements inside the tokio::spawn body, not on the doc comment above the function. Unmutated: PASS [6.526s]. Mutated, verbatim, on BOTH tries: `thread 'dangerous_expiry_cancels_production_streaming_bash_process_tree' (3184477) panicked at crates/wcore-agent/tests/dangerous_lease_e2e_test.rs:218:14: / lease expiry must stop the production Bash dispatch within its cancellation budget: Elapsed(())` / `TRY 2 FAIL [  10.187s] (1/1) wcore-agent::dangerous_lease_e2e_test dangerous_expiry_cancels_production_streaming_bash_process_tree` / `Summary [  20.380s] 1 test run: 0 passed, 1 failed, 3862 skipped`. It reddens at abort_by = granted + LEASE_TTL + CANCELLATION_BUDGET = 10s, which is the bound the criterion is about, and it reddens on the timeout rather than on a later assertion -- the run future never returns at all, exactly as the structural argument predicted."
  - id: c3
    text: "Artificially delaying pid publication no longer turns the test red"
    state: met
    evidence: "symbol:crates/wcore-agent/tests/dangerous_lease_e2e_test.rs::read_pid"
    owner: core
    note: "MUTATION ARM RUN, and the residual the previous pass named is now MEASURED. The fixture script (line 169, a format! operand -- CODE, verified by reading the line back after the edit) was changed to `echo streaming-proof; sleep 2; echo $$ > '{}'; sleep 30 & echo $! > '{}'; wait`, artificially delaying pid publication by 2s. The test PASSES: `PASS [   6.247s]` and, on the instrumented repeat, `PASS [   6.268s]`. An instrumented run reports the headroom directly: `[HEADROOM] bootstrap+init consumed 774.862961ms of the 4s TREE_UP_BUDGET before the fixture script could run` and `[HEADROOM] both pids read at 3.493477069s after grant`. So bootstrap costs 774.86ms of TREE_UP_BUDGET, un-delayed publication lands at about 1.49s, and the room for an artificial delay is about 2.5s, not the full 4s -- the residual is real, it is bounded, and 2s fits inside it with 506ms to spare. THE 4s BOUND WAS THE DEFECT, shown on the same run: that delayed test took 6.268s wall, 2.27s past the `assert!(started.elapsed() < Duration::from_secs(4))` that dc7aa695 deleted. Under the old shape this delay was red; under the new shape it is green. The instrumentation was removed and the file restored to a clean git diff before anything was committed."
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
