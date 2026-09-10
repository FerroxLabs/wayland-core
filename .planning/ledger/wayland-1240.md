---
issue: 1240
repo: FerroxLabs/wayland
kind: defect
title: "await_completion_returns_on_match reds the shared-process lib leg on a timing race, not a process global"
status: open
last_verified_commit: 88a87e5ee
criteria:
  - id: c1
    text: "The rate is MEASURED on the containerised CI image -- not on hetzner-dsm, whose four local passes the issue itself says do not exonerate the leg -- at --retries 0 over N of at least 20, and the rate is recorded."
    state: not-met
    owner: core
    note: "STILL NOT MET, and deliberately not claimed at a lower bar. The criterion names ONE instrument -- the containerised CI image (`wayland-core-ci:rust-1.95-slim-bookworm`, built inline at .github/workflows/ci.yml:2032) -- at --retries 0 over N>=20, and it excludes hetzner-dsm by name. This lane can reach hetzner-dsm only, through tools/remote-proof.py, which runs one cargo command on the bare host and cannot build or drive that image. WHAT WAS ACHIEVED INSTEAD, stated so it is not mistaken for the measurement owed: on hetzner-dsm at loadavg 26-29 the full shared-process `cargo test -p wcore-agent --lib` leg ran 2727 passed / 0 failed in 25.38s at 88a87e5ee, and `agents::observer` ran 5/5 in 0.06s. Those are hetzner passes of exactly the kind the issue body says do not exonerate the leg. WHAT IS OWED: build the CI image and run `cargo test -p wcore-agent --lib agents::observer::tests::await_completion_returns_on_match` inside it, --retries 0, N>=20, recording the rate. NOTE THE ORDER: c2 is closed by its FIRST branch (the test no longer races real elapsed time), which its own text makes independent of c1 -- c1 gates only the allowlist branch, which was not taken."
  - id: c2
    text: "Either the test stops racing real elapsed time (assert a count, not a duration, the fix wayland#1182 applied to the workspace-walk control), or it is carried in .config/flaky-allowlist.txt WITH the rate c1 measured. That file's discipline is that an entry states what it measured, so an entry without c1 does not close this."
    state: met
    evidence: "test:crates/wcore-agent/src/agents/observer.rs::await_completion_returns_on_match"
    owner: core
    note: "MET at 88a87e5ee by the FIRST branch of this criterion -- the test stops racing real elapsed time -- so the allowlist branch, and with it the c1 rate it would require, is not taken. WHAT THE RACE WAS: the waiter's 500 ms deadline starts inside the spawned task, and the publish is gated behind an `await_subscribers` loop whose 1 ms `tokio::time::sleep` is a request, not a guarantee; on the shared-process lib leg a late wakeup lets the deadline expire before `Completed` is ever published. WHAT CHANGED: `#[tokio::test(start_paused = true)]` (tokio `test-util`, added to wcore-agent [dev-dependencies]) puts the test on a VIRTUAL clock that advances only to the next armed timer when every task is idle, so virtual time steps to the 1 ms poll and never past it to the 500 ms deadline; and `await_subscribers` is now bounded by a COUNT OF POLLS (10_000) rather than a `std::time::Instant` deadline, with `assert_eq!(receiver_count, 1)` as the explicit gate. This is the wayland#1182 shape the criterion names: assert a count, not a duration. MEASURED: 5/5 in 0.06s wall on hetzner-dsm -- a 500 ms deadline that costs no wall time at all is the observable signature that no real clock is being raced. LIMIT: this closes the CONSTRUCTION. It does not measure the old rate, which is c1 and remains owed."
  - id: c3
    text: "A red arm is shown: after the change, the test still fails when the observer genuinely does not see Completed. A deadline made unreachable passes for the wrong reason."
    state: met
    evidence: "test:crates/wcore-agent/src/agents/observer.rs::await_completion_times_out_when_completed_is_suppressed"
    owner: core
    note: "MET at 88a87e5ee, BOTH as a one-off mutation and as a permanent in-tree control, because the criterion's real worry -- a deadline made unreachable passes for the wrong reason -- is a property that has to keep holding. MUTATION ARM (asserted to have landed on CODE before the run: `git diff` showed the six deleted lines of the `bus.publish(AgentMessage::Completed {..})` call, not a comment): at dccd700ee `cargo test -p wcore-agent --lib agents::observer` on hetzner-dsm gave `test result: FAILED. 4 passed; 1 failed`, payload `observer.rs:372:9: waiter must resolve on the matching Completed event, got Err(Timeout)`, finished in 0.06s. So the paused clock reaches the 500 ms deadline and reports Timeout -- it is not disarmed. GREEN ARM at 88a87e5ee: 5 passed / 0 failed, same command, same host. PERMANENT CONTROL: `await_completion_times_out_when_completed_is_suppressed` is the same construction with `Completed` withheld, asserting `Err(AgentBusError::Timeout)`, so a future change that disarms the timer reds here instead of quietly making the green arm vacuous."
  - id: c4
    text: "The required Shared-process lib suite check is left able to mean something: whatever closes this does not teach a reader to discount a red on that leg."
    state: met
    evidence: "absent:.config/flaky-allowlist.txt::await_completion_returns_on_match"
    owner: core
    note: "MET at 88a87e5ee. NOTHING here teaches a reader to discount a red on the shared-process lib leg: no flaky-allowlist entry was written for this test (the `absent:` anchor re-reads that file every gate run, so an entry added later reds this criterion), no retry override was added to .config/nextest.toml, and the test was not serialized or moved off the leg. It still runs in the same shared process as the other 2726 wcore-agent lib tests. POSITIVELY CONTROLLED rather than argued: the leg still FAILS when the property fails -- c3's mutation arm reddened `cargo test -p wcore-agent --lib` itself, which is the exact invocation ci.yml:2490 runs -- and it PASSES on the unmutated tree, 2727 passed / 0 failed / 3 ignored in 25.38s at 88a87e5ee on hetzner-dsm, --no-fail-fast. Graded with `cargo test`, never `cargo nextest`: nextest gives every test its own process and the whole shared-process hazard class is invisible under it."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

observer.rs:303 is assert!(matches!(got, Ok(AgentMessage::Completed { .. }))),
so the waiter one line above returned the timeout arm. Not contamination,
despite being surfaced by the leg built for that class.
