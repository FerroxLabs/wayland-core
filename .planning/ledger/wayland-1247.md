---
issue: 1247
repo: FerroxLabs/wayland
kind: defect
title: "wcore-swarm worktree linux tests fail under full-workspace load, reddening ci-linux for unrelated lanes"
status: open
last_verified_commit: cdd15c3dd
criteria:
  - id: c1
    text: "Both named deadlines are addressed as a FAMILY: linux.rs:693 (read_child_pid's 3 s poll) and linux.rs:972. The issue found a second test on the first reproduction attempt, so fixing the one CI named would close the instance and leave the class."
    state: met
    evidence: "commit:2347d8f9c"
    owner: core
    note: "MET at 509f4426b. Both named deadlines were addressed in ONE change, which is what this criterion asks: 2347d8f9c fixes linux.rs:693 (`read_child_pid`, the 3s poll, now a 25s liveness backstop over a fixture record that carries its own terminator) and linux.rs:972 (`worktree_add_timeout_kills_tree_and_reports_preserved_residual`, stage pinned and budget re-derived) in the same commit. Anchored on the commit because the criterion property is that the two were treated as one family, which no single file token can express. VERIFIED IN THE TREE, not read off the message: `read_child_pid` at linux.rs:726 now uses `Duration::from_secs(25)`, and the config-stage pin is at linux.rs:1023-1032. NOT GRADED: c4 -- the family is NOT closed. `wait_until_process_gone` still carries a hard-coded `Duration::from_secs(3)` at linux.rs:638, so a grep over that file does not come back clean."
  - id: c2
    text: "linux.rs:972 is fixed at its cause -- the 200 ms git timeout fires at the git config safety check stage instead of the intended worktree add stage, so no residual path exists yet when the second assertion runs -- and not by widening the budget."
    state: met
    evidence: "test:crates/wcore-swarm/src/worktree_tests/linux.rs::config_safety_check_outlives_a_short_worktree_budget"
    owner: core
    note: "MET at cdd15c3dd, AT THE CAUSE THIS CRITERION NAMES AND NOT BY WIDENING THE BUDGET. The cause: `reject_executable_checkout_config` ran `capture_bounded_process(cmd, self.capture_limits, ..)`, i.e. the git config safety check spent the CALLER'S operation budget, and it runs first -- so at 200 ms a loaded runner burned the whole budget on that stage's process spawn, `mkdir -p .swarm-worktrees/worker-1` never ran, and the residual assertion failed while the `timed out` assertion still passed. THE FIX: `config_check_limits()` in worktree_cleanup.rs gives the check `self.capture_limits.timeout.max(CONFIG_CHECK_MIN_TIMEOUT)` with the floor at 30 s. `max`, so it can only RAISE: production passes GIT_CAPTURE_LIMITS (120 s), which is already above the floor, so production behaviour is unchanged in both directions. The test budget under it was NOT raised -- it is the same 2 s that was already there, and it now bounds only the stage under test (spawn, mkdir, fork the grandchild, publish its pid). RED ARM, deterministic and load-independent rather than waiting for a loaded runner: `config_safety_check_outlives_a_short_worktree_budget` makes the config branch `sleep 3` per scope (2 scopes = 6 s) against that 2 s budget. At d90d78783, with only the `config_check_limits()` call reverted to `self.capture_limits` (`git diff` confirmed the single-line mutation landed on the call, not a comment), that test FAILED on hetzner-dsm: `19 passed; 1 failed`, payload `linux.rs:1120:5: the config stage was cut short by the operation budget: worktree io: git config safety check: process timed out after 2s` -- the issue's own failure, reproduced on demand. GREEN at cdd15c3dd: 20 passed / 0 failed in 8.11s. ALSO ADDED, because `the error is not a config timeout` is equally true of a run whose config stage never started: the fixture writes a `config.ack` marker and both tests assert it exists -- an EVENT, not an inferred duration."
  - id: c3
    text: "The measured failure rate is re-measured after the fix at N of at least 13 on hetzner-dsm, the same N that produced the 1-in-13 baseline on a quiet host, and recorded."
    state: met
    evidence: "test:crates/wcore-swarm/src/worktree_tests/linux.rs::worktree_add_timeout_kills_tree_and_reports_preserved_residual"
    owner: core
    note: "MET at cdd15c3dd. RE-MEASURED AFTER THE FIX at exactly the N the criterion names: N=13 consecutive `cargo test -p wcore-swarm --lib worktree::tests::linux::` runs on hetzner-dsm through tools/remote-proof.py, one cargo invocation per trial, no retries. RESULT: 13/13 `20 passed; 0 failed`, i.e. a measured rate of 0/13 against the 1-in-13 baseline. In-test duration 8.07-8.12s, a 0.05s spread. LOAD RECORDED BESIDE THE RATE, sampled from /proc/loadavg immediately before each trial: 1-min 25.84-28.59, 5-min 27.60-28.60, 15-min 27.98-28.32 on a 96-core host. STATED HONESTLY AND NOT ROUNDED UP: the 1-in-13 baseline was taken on a QUIET host and this host was not quiet, so this is the harder condition, not a matched one; and 13 trials with zero failures cannot distinguish 0% from the baseline's 7.7% with confidence -- the one-sided 95% upper bound on the true rate at 0/13 is about 21%. What N=13 does establish is that the fix did not make it worse and that the deterministic red arm in c2, not a rate, is what carries the causal claim."
  - id: c4
    text: "A grep or a test proves no other hard-coded short deadline remains in crates/wcore-swarm/src/worktree_tests/linux.rs, so the family is closed rather than the two instances that were noticed."
    state: met
    evidence: "test:crates/wcore-swarm/src/worktree_tests/linux.rs::no_hard_coded_short_deadline_remains_in_this_file"
    owner: core
    note: "MET at cdd15c3dd, by a TEST rather than a one-off grep, because a grep run once by a human closes today's instances and the whole complaint here is that they were found one per cycle. THREE remaining short deadlines were fixed, not just the two that had been noticed: `wait_until_process_gone` 3s -> 25s (linux.rs:653), the `cancelled_cleanup_kills_git_and_reports_residual` cleanup bound 1s -> 25s, and `fixture_git_output`'s setup budget 5s -> 25s. 25 s is `read_child_pid`'s already-measured figure: inside nextest's 60 s hard kill, so the failure still carries a diagnostic instead of a bare TIMEOUT. THE RULE THE TEST ENFORCES, and where the line falls: a `Duration::from_millis(..)` here is a poll interval (capped at 50 ms); a `Duration::from_secs(..)` is a deadline and must clear a 25 s liveness floor UNLESS it is spelled `timeout:`, i.e. a `CaptureLimits` budget the test is deliberately feeding the product as the behaviour under test. Comment lines are stripped before scanning, so a doc comment quoting a deadline is not graded as one. NOT VACUOUS, and controlled both ways: the scanner asserts it still finds >=3 deadlines, >=2 `timeout:` budgets and >=2 poll intervals (actuals 3 / 4 / 3), so a scan that stops matching this file reds instead of certifying it; and at dddccbcf7, with `wait_until_process_gone` put back to `from_secs(3)`, it FAILED on hetzner-dsm naming `linux.rs:653: Duration::from_secs(3) is a deadline under the 25s liveness floor`."
  - id: c5
    text: "The polling mitigation already applied to try_read_child_pid is not counted as the fix: its own doc comment records that it reduced the rate and the 3 s budget still loses under CI load."
    state: met
    evidence: "file:crates/wcore-swarm/src/worktree_tests/linux.rs:723:refuses an unterminated record"
    owner: core
    note: "MET at 509f4426b. The polling mitigation is explicitly NOT what closed this. The fixtures now publish the pid with a shell builtin `echo`, which supplies its own newline, and `try_read_child_pid` REFUSES a record with no terminator (linux.rs:687 and the doc comment at :708), so a partial write can no longer be read as a short pid -- `1234` observed as `12` used to parse happily and name an unrelated process. The 3s budget was not merely widened either: the doc comment records that 25s was chosen to fit INSIDE nextest default 60s hard kill, measured both ways (at 60s the run reports only `TIMEOUT [60.005s]` with no diagnostic; at 25s it reports `FAIL [25.056s]` with the message that distinguishes a hang from a slow runner). WHAT WOULD FALSIFY THIS: the terminator refusal being removed, which the anchored line reds on."
---

Created 2026-08-31. This issue was filed 2026-08-29/30 by this cycle's own
verification, was in scope for the release gate from that moment, and had no
ledger file -- so scripts/check-release-readiness.py, which reads ledger files
and nothing else, could not count it. CI runs the coverage arm with --offline,
which is the arm that would have said so.

Its body declared no acceptance criteria, so it could not have been closed as
filed either. The criteria above are AUTHORED from measurements the body
already records.

report is a required status context depending on ci-linux, so a red here fails
the required check for whatever lane happens to be pushing -- on a crate that
lane did not touch. That blast radius is the reason this is not a minor flake.
