---
issue: 449
repo: FerroxLabs/wayland-core
kind: defect
title: "[mutants-nightly] wcore-cron — surviving mutants (2026-09-04)"
status: open
last_verified_commit: 7f808e952
criteria:
  - id: c1
    text: "The surviving mutants reported for wcore-cron are dispositioned: each is either killed by a new or strengthened test, or recorded with a reason it is not worth killing."
    state: met
    evidence: "file:.planning/evidence/core-449-mutant-disposition/disposition.md:9:killed by a new or strengthened test"
    owner: core
    note: "MET. All 64 mutants `mutants-nightly` reported MISSED for wcore-cron are dispositioned one by one: 51 killed by a new or strengthened test that is named against the mutant it kills, 13 recorded as not worth killing with a reason specific to that row. Nothing is dispositioned as `hard to test`, `low value` or `covered indirectly`. The record is `.planning/evidence/core-449-mutant-disposition/disposition.md`, which carries every row with its mac and Linux outcome side by side. THE LIST GRADED IS 64, NOT 55. The earlier pass worked to 55; the two are the same list, not two lists. The 64 lines of run 34438249721's `missed.txt` (artifact `mutants-log-wcore-cron` id 10138518656, headSha fdf4b1e1c) were diffed line for line against the 64 this ledger already recorded from run 33844721279 and the sets are IDENTICAL, so the mutant set is stable across the two runs; 55 is what remains after subtracting the 9 rows behind a `cfg` the reporting host does not compile (7 in `#[cfg(windows)] mod sys`, 2 in the `#[cfg(not(any(unix, windows)))]` stub). This lane dispositions all 64 rather than the 55, because `the host compiled it out` is itself a reason that has to be written down and a denominator is where such rows stop being looked at. WHAT WAS KILLED, AND BY WHAT. 29 rows were already covered by the previous pass's tests (store.rs 19, trigger.rs 8, events.rs 202:5, retry.rs 71:25) and this run is the first time any of them has been SHOWN to die rather than asserted to. 22 rows are newly killed here: lease.rs 147:9 (is_owner on a REFUSED attempt -- every prior assertion was on an owner, where a constant `true` is indistinguishable), 205:9 x2, 455:9 and 579:5 x2; history.rs 122:19; runner.rs 73:9, 78:12, 82:9 (the TestClock, which is the instrument every schedule test reads), 286:9, 359:9, 592:9, 606:9, 1068:31, 1086:31, 1101:20 and 1118:19; and store.rs 274:49 x2, 274:57 and 430:20. THE FOUR warn!-ONLY ROWS ARE KILLED, NOT EXCUSED. The previous pass recorded store.rs 274:49 x2, 274:57 and 430:20 as unobservable `without a subscriber harness and a dev-dependency this lane did not add`. The dev-dependency turns out not to be needed: `tracing` itself exposes the `Subscriber` trait and a thread-local `subscriber::set_default`, and every test in `tests/diagnostic_warnings.rs` is a current-thread tokio runtime, so the code under test runs on the thread that installed the capture. The capture carries its own positive control (`the_capture_actually_sees_a_warning_from_this_crate`), because a capture that silently sees nothing satisfies every `no warning was emitted` assertion in the file. Cargo.lock is untouched. THE 13 RECORDED ROWS, none of which share a reason. lease.rs 365:9 (`release` -> `()`) is semantically equivalent: `release` takes `self` BY VALUE, so the mutant still drops `self` at the end of the call, and `Drop` opens with the identical `handle.revoke()` -- the mutant's whole observable sequence IS Drop's. lease.rs 442:64 (`|` -> `^`) is equivalent by value: LOCK_EX=2 and LOCK_NB=4 are disjoint single bits, so `2|4` and `2^4` are both 6 and the `flock(2)` call is bit-for-bit the same. lease.rs 448:27 (guard -> `true`) is unreachable: it only diverges for a `flock` errno other than EAGAIN/EWOULDBLOCK, and the descriptor is one this module opened itself on a regular sentinel file it created, with a constant operation -- EBADF and EINVAL are impossible by construction, so the `Err` arm is defensive rather than reachable. lease.rs 448:32 (`== EAGAIN_LINUX` -> `!=`) is MEASURED, not argued: it is CAUGHT on Linux by `a_second_attempt_in_one_process_is_refused` and exactly equivalent on macOS/BSD, where a contended `flock` returns 35 so the negated comparison is false either way -- and the control that makes this a measurement is the mirror row lease.rs 448:56, which is caught on macOS and missed on Linux, the same test with the constants swapped. The 7 rows at lease.rs 503-540 are inside `#[cfg(windows)] mod sys` and the reporting run was `macos-latest`, so the mutation could not change the binary the tests ran; the control is that the same 7 are missed on the Linux run too, on a tree whose lease tests were strengthened, which is what a host artifact looks like and is not what a test gap looks like. Grading them needs a Windows-hosted mutation leg, which the nightly matrix does not have (every job is `runs-on: macos-latest`) -- a core#424 matrix gap, recorded as such and NOT counted as killed. The 2 rows at lease.rs 564:9 are the `#[cfg(not(any(unix, windows)))]` stub, compiled on no target this repository builds, and that disposition is final. THE 2 TIMEOUTS ARE STILL NOT COUNTED AS ANYTHING. runner.rs 202:19 and 239:19 are not in the 64, are not claimed as killed, and are not folded into a catch rate; the mutated index arithmetic does not terminate, so the suite is stopped by the per-mutant clock rather than by an assertion. That is the same convention core#424 used when it published 74.8% rather than 75.4%. HOW THE KILLS WERE SHOWN. Green arm: `cargo test --locked -p wcore-cron` on the Linux proof host, 158 tests, 0 failures, receipt remote_exit 0 and complete true. Red arms: `cargo mutants -p wcore-cron --no-shuffle --timeout-multiplier 5 --minimum-test-timeout 90 -j 1` over the same tree -- cargo-mutants applies each mutant from the same span that produced the reported row, builds, runs the suite and reverts, so red and green come from the instrument that produced the list. Run A, source dafb3cc7c, receipt remote_exit 3 and complete true: 339 mutants tested in 16m -- 15 missed, 245 caught, 77 unviable, 2 timeouts. Run B, source 7f808e952, receipt remote_exit 0 and complete true: `--re 'runner\.rs:(592|606):'`, 2 mutants tested in 17s, 2 caught. Run B exists because `tests/runner_lifecycle.rs` landed after run A, so those two rows are a red arm AND a green arm rather than a single reading: runner.rs 592:9 and 606:9 are MISSED in run A on the tree without that file and CAUGHT in run B on the tree with it, and nothing else differs between the commits. AGAINST THE REPORTING RUN, ALL 339 ROWS: the two runs generate the identical mutant set, and the outcomes line up 195 caught->caught, 77 unviable->unviable, 2 timeout->timeout, 50 missed->caught, 14 missed->missed, and exactly ONE caught->missed -- lease.rs 448:56, the errno mirror the analysis predicts in advance. One disagreeing row out of 339, and it is the predicted one, is what makes a Linux measurement admissible as evidence about a macOS-reported list; the Linux run also reproduces the reporting run's unviable count (77) and its timeout set (the same two rows) exactly. `-j 1` IS LOAD-BEARING and a first attempt at `-j 6` was discarded: the proof harness exports one CARGO_TARGET_DIR for the whole slot, so parallel cargo-mutants workers copy the tree to separate directories but write their build artifacts to the same target dir and overwrite each other's test binaries. That run reported runner.rs 1118:19 -- whose only effect is one field of one `debug!` record -- as caught by a cron-EXPRESSION-PARSING test, and reported retry.rs 71:25 as missed even though `a_backoff_ceiling_clamps_to_a_full_day` asserts the `24 * 3600` clamp lands on 86400 and provably reddens it. Both directions were wrong, so nothing from it is used here."
  - id: c2
    text: "core#424 is re-graded against this run, since its recorded premise -- that mutants-nightly has produced zero data across 87 runs -- is refuted by a run that produced 339 mutants."
    state: met
    evidence: "file:.planning/ledger/wayland-core-424.md:20:0 of 94 mutants-nightly runs have event=workflow_dispatch"
    owner: core
    note: "MET at da51d7a59. core#424 has now been re-graded against run 33844721279 TWICE, and the second pass moved a criterion DOWN, which is what distinguishes a re-grade from a re-quote. FIRST PASS (2026-09-04) graded c3 and c4 met from that run`s artifacts and left c5 not-met. SECOND PASS (2026-09-10, this one) read wayland-core#424`s acceptance section back off the tracker and found that this ledger`s copy of c2 had been broadened: the issue says `one workflow_dispatch run`, the ledger said `a run`, and the run it was graded against is event=schedule. c2 is therefore REGRADED not-met, with the owed run stated exactly. So the re-grade is real in both directions: the premise `zero data across 87 runs` IS refuted -- 339 mutants, 64 missed, read from the artifact and re-confirmed today against a 64-line missed.txt -- and producing data still does not satisfy every criterion core#424 carries. That was the caution this criterion was written with (`producing data is necessary for its criteria, not obviously sufficient`) and it turned out to be the right one. Re-graded FROM THE ARTIFACT, as this criterion requires: `gh run download 33844721279` then read .blackboard/E2E-MUTATION-BASELINE/wcore-cron.log and mutants.out/. The job log was not used for any part of it; core#424`s own c4 records why -- it returns 10 hits for the summary line where the truth is 0, because it echoes the workflow`s format comment once per leg. ANCHOR: the fragment below is the measurement that forced the downgrade, and it lives in core#424`s ledger, not this one; delete it and this criterion reds."
---

# The complete 64, from the artifact -- not the 20 the issue body shows

Source: run 33844721279, artifact `mutants-log-wcore-cron` (id 9932557850),
`target/mutants-wcore-cron/mutants.out/missed.txt`, exactly 64 lines, downloaded
2026-09-10. Paths are relative to `crates/wcore-cron/src/`. Nothing here is
killed; this is the inventory c1 has to disposition, with the rows that no test
on this host could ever have killed separated out and labelled as such.

## Live on the host that ran them -- 55 genuine survivors

    events.rs:202:5: replace restrict_permissions with ()
    history.rs:122:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in read_lines
    lease.rs:147:9: replace LeaseAttempt::is_owner -> bool with true
    lease.rs:205:9: replace LeaseHandle::owner_pid -> u32 with 0
    lease.rs:205:9: replace LeaseHandle::owner_pid -> u32 with 1
    lease.rs:365:9: replace ScheduleLease::release with ()
    lease.rs:442:64: replace | with ^ in sys::try_lock_exclusive
    lease.rs:448:27: replace match guard code == EAGAIN_LINUX || code == EWOULDBLOCK_BSD with true in sys::try_lock_exclusive
    lease.rs:448:32: replace == with != in sys::try_lock_exclusive
    lease.rs:455:9: replace sys::unlock with ()
    lease.rs:579:5: replace default_lease_dir -> Option<PathBuf> with None
    lease.rs:579:5: replace default_lease_dir -> Option<PathBuf> with Some(Default::default())
    retry.rs:71:25: replace * with + in RetryPolicy::clamped
    runner.rs:73:9: replace TestClock::advance with ()
    runner.rs:78:12: replace += with -= in TestClock::advance
    runner.rs:82:9: replace TestClock::set with ()
    runner.rs:286:9: replace && with || in scan_target_text
    runner.rs:359:9: replace JobHandler::dispatch_is_idempotent -> bool with true
    runner.rs:592:9: replace CronRunner::shutdown with ()
    runner.rs:606:9: replace <impl Drop for CronRunner>::drop with ()
    runner.rs:1068:31: replace > with >= in drain_published_events
    runner.rs:1086:31: replace < with <= in drain_published_events
    runner.rs:1101:20: delete ! in drain_published_events
    runner.rs:1118:19: replace += with *= in drain_published_events
    store.rs:35:5: replace default_store_path -> Option<PathBuf> with None
    store.rs:35:5: replace default_store_path -> Option<PathBuf> with Some(Default::default())
    store.rs:44:5: replace default_history_path -> Option<PathBuf> with None
    store.rs:44:5: replace default_history_path -> Option<PathBuf> with Some(Default::default())
    store.rs:68:9: replace CronStore::list_for_run -> Result<Vec<CronJob>> with Ok(vec![])
    store.rs:79:9: replace CronStore::cron_dir -> Option<PathBuf> with Some(Default::default())
    store.rs:145:5: replace set_owner_only_perms with ()
    store.rs:171:19: replace ^= with |= in keyed_hash_hex::fnv1a
    store.rs:219:9: replace FileCronStore::integrity_key -> Option<Vec<u8>> with Some(vec![])
    store.rs:219:9: replace FileCronStore::integrity_key -> Option<Vec<u8>> with Some(vec![0])
    store.rs:219:9: replace FileCronStore::integrity_key -> Option<Vec<u8>> with Some(vec![1])
    store.rs:246:9: replace FileCronStore::check_ownership_and_perms -> Result<()> with Ok(())
    store.rs:251:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in FileCronStore::check_ownership_and_perms
    store.rs:264:46: replace & with | in FileCronStore::check_ownership_and_perms
    store.rs:264:46: replace & with ^ in FileCronStore::check_ownership_and_perms
    store.rs:265:25: replace != with == in FileCronStore::check_ownership_and_perms
    store.rs:265:17: replace & with | in FileCronStore::check_ownership_and_perms
    store.rs:265:17: replace & with ^ in FileCronStore::check_ownership_and_perms
    store.rs:274:57: replace != with == in FileCronStore::check_ownership_and_perms
    store.rs:274:49: replace & with | in FileCronStore::check_ownership_and_perms
    store.rs:274:49: replace & with ^ in FileCronStore::check_ownership_and_perms
    store.rs:362:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in FileCronStore::read_file
    store.rs:430:20: delete ! in <impl CronStore for FileCronStore>::list_for_run
    trigger.rs:285:57: replace < with == in Trigger::validate
    trigger.rs:285:57: replace < with <= in Trigger::validate
    trigger.rs:332:24: replace > with >= in Trigger::next_after
    trigger.rs:339:28: replace + with - in Trigger::next_after
    trigger.rs:359:28: replace + with - in Trigger::next_after
    trigger.rs:370:24: replace match guard bound.is_spent(t) with false in Trigger::next_after
    trigger.rs:378:9: replace Trigger::is_clock_driven -> bool with false
    trigger.rs:453:12: replace > with >= in heartbeat_state

## Compiled out on `macos-latest`: `#[cfg(windows)]`, lease.rs 461-555 -- 7

Ungraded, NOT caught and NOT equivalent. A mutant in a block the host does not
compile cannot fail a test there, so its MISSED status says nothing about the
tests. Saying anything about these needs a Windows mutation leg.

    lease.rs:503:9: replace sys::try_lock_exclusive -> std::io::Result<bool> with Ok(true)
    lease.rs:503:9: replace sys::try_lock_exclusive -> std::io::Result<bool> with Ok(false)
    lease.rs:521:41: replace | with & in sys::try_lock_exclusive
    lease.rs:521:41: replace | with ^ in sys::try_lock_exclusive
    lease.rs:528:15: replace != with == in sys::try_lock_exclusive
    lease.rs:533:13: delete match arm Some(ERROR_LOCK_VIOLATION) | Some(ERROR_IO_PENDING) in sys::try_lock_exclusive
    lease.rs:540:9: replace sys::unlock with ()

## Never built anywhere: `#[cfg(not(any(unix, windows)))]`, lease.rs 556-578 -- 2

The unsupported-platform fallback. Unreachable on every target this repository
builds, so no nightly on any host can grade them. The honest disposition is
"not worth killing", and it is the one disposition here that is already final.

    lease.rs:564:9: replace sys::try_lock_exclusive -> std::io::Result<bool> with Ok(true)
    lease.rs:564:9: replace sys::try_lock_exclusive -> std::io::Result<bool> with Ok(false)

## The 2 timeouts, which are neither caught nor missed

    runner.rs:202:19: replace + with - in contains_at_command_position
    runner.rs:239:19: replace + with - in contains_env_filename

Both hit the per-mutant test timeout at 90.03s and 90.07s against a 5s baseline,
so they are harness outcomes, not test-quality outcomes. Under the derived
timeout that replaced `--timeout 90` (core#451, da51d7a59) their bound becomes
`max(90, 5 x measured baseline)`, so if they were near-misses they should resolve
one way or the other on the next nightly.

## Where the 55 actually cluster

| file | live survivors | what they touch |
|---|---|---|
| `store.rs` | 23 | `check_ownership_and_perms` (9), `integrity_key` (3), `fnv1a`, default paths, `read_file` NotFound guard |
| `lease.rs` | 10 | `is_owner`, `owner_pid`, `release`, and the Unix `flock` path |
| `runner.rs` | 11 | `TestClock`, `shutdown`, `Drop`, `drain_published_events` bounds |
| `trigger.rs` | 8 | `validate`, `next_after` arithmetic, `heartbeat_state` |
| `events.rs` | 1 | `restrict_permissions` |
| `history.rs` | 1 | `read_lines` NotFound guard |
| `retry.rs` | 1 | `RetryPolicy::clamped` |

The lock cluster the original note flagged is real and is now 10 rows rather
than 19 -- the other 9 were the Windows and fallback backends. But `store.rs` is
the bigger hole and nobody had seen it, because it does not appear in the top
20: nine mutants inside `check_ownership_and_perms` survive, including replacing
the whole function with `Ok(())`, and three inside `integrity_key` survive,
including returning an empty key. Those are the file-permission and
tamper-detection checks. Under-tested locking is the wrong place to be blind;
an under-tested "is this file mine and is it 0600" check is worse.

# An auto-filed mutation-testing result, ledgered for coverage, untriaged

Filed by automation mid-swarm on 2026-09-04. This file makes no claim about the
surviving mutants; it records what the run reported and why it bears on core#424.

Worth a human eye before the cut, though it is not itself a release blocker: the
missed mutants cluster hard in `crates/wcore-cron/src/lease.rs`, and specifically
in `sys::try_lock_exclusive` and `sys::unlock` -- replacing `unlock` with `()`
survives, and `try_lock_exclusive` survives being replaced by both `Ok(true)` and
`Ok(false)`. That is the file-locking path, and this repo has already shipped one
measured lock defect of exactly that family (a data-file lock duplicated through
`fork()`, refusing 47.6% of reopens under load). Under-tested locking code is not
proof of a second such defect, but it is the wrong place to be blind.

Structural note, the same one core#443 carries: an automated nightly can red the
release gate at any hour purely by filing, because coverage scopes EVERY open issue
on either tracker. Two fired during this one session, four hours apart. `ci.yml`
runs the checker `--offline`, which skips coverage and divergence entirely, so the
class is invisible on every PR and surfaces only when a release is attempted.
