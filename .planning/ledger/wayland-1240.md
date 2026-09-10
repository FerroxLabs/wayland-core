---
issue: 1240
repo: FerroxLabs/wayland
kind: defect
title: "await_completion_returns_on_match reds the shared-process lib leg on a timing race, not a process global"
status: open
last_verified_commit: 782efce3a
criteria:
  - id: c1
    text: "The rate is MEASURED on the containerised CI image -- not on hetzner-dsm, whose four local passes the issue itself says do not exonerate the leg -- at --retries 0 over N of at least 20, and the rate is recorded."
    state: met
    evidence: "file:.planning/evidence/load-conditioned-flakes/RATES.md"
    owner: core
    note: "MET at 14aa2f1ca, ON THE INSTRUMENT THIS CRITERION NAMES. FIRST, A CORRECTION TO THE PREVIOUS NOTE: it claimed this lane can reach hetzner-dsm only and `cannot build or drive that image`. That is FALSE -- hetzner-dsm has Docker 29.2.1 installed. The image was built there from the inline Dockerfile in the `Build CI image` step of .github/workflows/ci.yml and tagged `wayland-core-ci:rust-1.95-slim-bookworm`, the workflow's own CI_IMAGE. Identified FROM INSIDE the container rather than assumed: rustc 1.95.0 (59807616e 2026-04-14), cargo 1.95.0, cargo-nextest 0.9.143, Debian GNU/Linux 12 (bookworm). The source was materialised by `git archive 14aa2f1ca`, so the tree under test is that commit exactly and not a working copy. MEASURED: 20 runs of `cargo test -p wcore-agent --lib --no-fail-fast` in that image with the workflow's own DOCKER_RUN_SANDBOX grants -- the SHARED-PROCESS leg, 2733 tests in ONE process, and `cargo test` has no retry mechanism so every run is --retries 0 by construction -- gave 20/20 `ok. 2733 passed; 0 failed` in 29.77-32.89 s, at 1-min loadavg 142.69-198.23. RATE: 0/20, with the load figure recorded beside it. NOT A DEAD INSTRUMENT, controlled in the SAME image with the SAME command: with the `bus.publish(AgentMessage::Completed {..})` call deleted from the test body (`diff` confirmed the six removed lines were CODE, not a comment) the leg reported `FAILED. 2732 passed; 1 failed` with `observer.rs:370:9: waiter must resolve on the matching Completed event, got Err(Timeout)`; restored byte-identical, `touch`ed so cargo could not skip the rebuild, and re-run in the same image it is `ok. 2733 passed; 0 failed` again. LIMIT, stated so the number is not over-read: a 0/20 rate on the POST-c2 tree measures the tree c2 already fixed. The test is on tokio's virtual clock, so it cannot race real elapsed time and a zero rate is exactly what the fix predicts. This does NOT recover the pre-fix rate the issue was filed on -- nothing in this criterion asks for that, and nothing here claims it. Per-run table in .planning/evidence/load-conditioned-flakes/RATES.md."
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
