---
issue: 337
repo: FerroxLabs/wayland-core
title: "Fragile 4s wall-clock bound: dangerous_expiry_cancels_production_streaming_bash_process_tree"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "The wall-clock assertion bounds cancellation only, not bootstrap plus pid publication plus cancellation"
    state: not-met
    owner: core
    note: "the clock starts before the run is spawned, two read_pid calls with 2s timeouts each may legally burn 4s inside that window, and the cancellation itself has its own 4s timeout - so the nested budgets permit 8s while the assertion demands under 4s. It is structurally unsatisfiable, not a tight margin"
  - id: c2
    text: "A mutation that stops lease expiry cancelling the process tree still turns the test red"
    state: not-met
    owner: core
    note: "the real coverage is the 4s timeout on the run future, the UserAborted match and the wait_gone check. The redundant elapsed assertion adds no coverage, only a race, so deleting it must not cost the test its teeth"
  - id: c3
    text: "Artificially delaying pid publication no longer turns the test red"
    state: not-met
    owner: core
    note: "today a 2.5s sleep before writing the pid files fails the test even though nothing about cancellation changed. That is the discriminating arm between an instrument bug and a product bound"
  - id: c4
    text: "The flaky-allowlist entry for this test is deleted rather than renewed"
    state: not-met
    owner: core
    note: "the entry expires 2026-10-15 and cites the read_pid Elapsed failure at line 97, not the 4s bound this issue reports. Anyone renewing it would renew against the wrong mechanism, and the entry says the fix is out of scope while the fix is a one-line re-baseline"
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
