core#449 c1 -- the disposition of all 64 reported wcore-cron survivors
========================================================================

Every mutant `mutants-nightly` reported MISSED for `wcore-cron` gets exactly
one disposition here: killed by a named test, or recorded with a reason that
is specific to it. Nothing is dispositioned as "hard to test", "low value"
or "covered indirectly".

    killed by a new or strengthened test .... 51
    recorded as not worth killing .......... 13
    total .................................. 64


THE LIST THIS GRADES, AND WHY IT IS 64 AND NOT 55
------------------------------------------------------------------------

Source: FerroxLabs/wayland-core actions run 34438249721, leg `mutants /
wcore-cron`, headSha fdf4b1e1c, artifact `mutants-log-wcore-cron` id
10138518656, file `target/mutants-wcore-cron/mutants.out/missed.txt`, exactly
64 lines. Host `macos-latest`. Run summary: 339 mutants tested in 28m -- 64
missed, 196 caught, 77 unviable, 2 timeouts.

An earlier pass worked to a figure of 55. The two are the same list, not two
lists: the 64 lines of this artifact were diffed line-for-line against the 64
the ledger records from the earlier run 33844721279 and the sets are
identical, and 55 is what remains after subtracting the 9 rows that sit
behind a `cfg` the reporting host does not compile. So

    64 reported missed
     - 7  inside `#[cfg(windows)] mod sys`            (lease.rs:461-555)
     - 2  inside `#[cfg(not(any(unix, windows)))]`    (lease.rs:556-578)
    = 55  live on the host that ran them

This file dispositions all 64 rather than the 55. A mutant that the host
could not grade still needs a recorded reason -- "the host compiled it out"
IS the reason, and burying those 9 in a denominator is how they stop being
looked at.


THE 2 TIMEOUTS ARE NOT COUNTED AS ANYTHING
------------------------------------------------------------------------

    runner.rs:202:19: replace + with - in contains_at_command_position
    runner.rs:239:19: replace + with - in contains_env_filename

They are not in the 64 and are not claimed as killed. A timeout is a harness
outcome, not a test-quality outcome: the mutated arithmetic walks an index
backwards and the scan does not terminate, so the suite is killed by the
per-mutant clock rather than by an assertion. Counting them as caught is what
would turn 74.8% into 75.4%, and the core#424 grading deliberately published
the lower figure for exactly this reason. That convention is kept here.

MEASURED, and this is the cross-host control that matters most: the
single-worker Linux run below reproduces the reporting run's timeout set
EXACTLY -- the same two rows, no others -- and its unviable count exactly
(77 and 77). Two hosts, two rustc versions, same 339 mutants, identical
harness outcomes.


HOW A KILL IS SHOWN
------------------------------------------------------------------------

A test that passes is not a kill. Every row marked KILLED below was put
through `cargo mutants -p wcore-cron` on the tree that contains the test, and
the row moved from `missed` to `caught` -- cargo-mutants applies the mutant
itself, from the same span that produced the reported row, builds, runs the
suite, and reverts, so the red arm and the green arm are the same instrument
that produced the list.

    green arm   `cargo test --locked -p wcore-cron` on the proof host,
                158 tests, 0 failures, receipt remote_exit 0 / complete true
    red arms    `cargo mutants -p wcore-cron --no-shuffle --timeout-multiplier 5
                 --minimum-test-timeout 90 -j 1`

    run A   source dafb3cc7c, receipt remote_exit 3 / complete true
            339 mutants tested in 16m: 15 missed, 245 caught, 77 unviable,
            2 timeouts
    run B   source 7f808e952, receipt remote_exit 0 / complete true
            `--re 'runner\.rs:(592|606):'` -- 2 mutants tested in 17s: 2 caught

Run B exists because `tests/runner_lifecycle.rs` landed after run A, so the
two rows it grades are a red arm and a green arm rather than one reading:
runner.rs:592:9 and runner.rs:606:9 are MISSED in run A, on the tree without
that file, and CAUGHT in run B, on the tree with it. Nothing else differs
between the two commits.

AGAINST THE REPORTING RUN, ROW BY ROW. All 339 mutants are the same in both
runs (the name sets are identical), so the outcomes can be compared directly:

    caught  -> caught     195
    unviable-> unviable    77
    timeout -> timeout      2
    missed  -> caught      50      the kills, run A
    missed  -> missed      14      12 recorded below, + the 2 run B closes
    caught  -> missed       1      lease.rs:448:56, the errno mirror

One row out of 339 disagrees between macOS and Linux, and it is the one the
EAGAIN/EWOULDBLOCK analysis predicts in advance. That is the reason a Linux
measurement is admissible as evidence about a macOS-reported list at all, and
it is stated here rather than assumed.

`-j 1` is not incidental. The proof harness exports one CARGO_TARGET_DIR for
the whole slot, so cargo-mutants workers running in parallel copy the tree to
separate directories but write their build artifacts to the SAME target dir
and overwrite each other's test binaries. A first attempt at `-j 6` produced
exactly that: `runner.rs:1118:19`, whose only effect is the value of a field
in one `debug!` record, was reported as caught by a cron-EXPRESSION-PARSING
test, and `retry.rs:71:25`, which is provably killed by an assertion that
`24 * 3600` clamps to 86400, was reported as missed. Those results were
discarded. Anything read off a parallel cargo-mutants run under a shared
target dir is noise in both directions.


KILLED -- 51 rows
------------------------------------------------------------------------

### events.rs -- 1

  events.rs:202:5: replace restrict_permissions with ()
      killed by  tests/store_hardening.rs::a_published_event_queue_is_tightened_to_owner_only
      mac 34438249721: missed      linux dafb3cc7c: caught

### history.rs -- 1

  history.rs:122:19: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in read_lines
      killed by  src/history.rs::tests::a_non_notfound_io_error_is_reported_rather_than_read_as_empty
      mac 34438249721: missed      linux dafb3cc7c: caught

### lease.rs -- 6

  lease.rs:147:9: replace LeaseAttempt::is_owner -> bool with true
      killed by  src/lease.rs::tests::a_second_attempt_in_one_process_is_refused
      mac 34438249721: missed      linux dafb3cc7c: caught

  lease.rs:205:9: replace LeaseHandle::owner_pid -> u32 with 0
      killed by  src/lease.rs::tests::a_handle_reports_the_pid_of_the_process_that_holds_it
      mac 34438249721: missed      linux dafb3cc7c: caught

  lease.rs:205:9: replace LeaseHandle::owner_pid -> u32 with 1
      killed by  src/lease.rs::tests::a_handle_reports_the_pid_of_the_process_that_holds_it
      mac 34438249721: missed      linux dafb3cc7c: caught

  lease.rs:455:9: replace sys::unlock with ()
      killed by  src/lease.rs::tests::a_duplicated_descriptor_does_not_pin_a_released_lease
      mac 34438249721: missed      linux dafb3cc7c: caught

  lease.rs:579:5: replace default_lease_dir -> Option<PathBuf> with None
      killed by  src/lease.rs::tests::the_default_lease_dir_is_the_directory_the_job_store_lives_in
      mac 34438249721: missed      linux dafb3cc7c: caught

  lease.rs:579:5: replace default_lease_dir -> Option<PathBuf> with Some(Default::default())
      killed by  src/lease.rs::tests::the_default_lease_dir_is_the_directory_the_job_store_lives_in
      mac 34438249721: missed      linux dafb3cc7c: caught

### retry.rs -- 1

  retry.rs:71:25: replace * with + in RetryPolicy::clamped
      killed by  tests/store_hardening.rs::a_backoff_ceiling_clamps_to_a_full_day
      mac 34438249721: missed      linux dafb3cc7c: caught

### runner.rs -- 11

  runner.rs:73:9: replace TestClock::advance with ()
      killed by  src/runner.rs::tests::the_test_clock_advances_forward_by_exactly_the_duration_given
      mac 34438249721: missed      linux dafb3cc7c: caught

  runner.rs:78:12: replace += with -= in TestClock::advance
      killed by  src/runner.rs::tests::the_test_clock_advances_forward_by_exactly_the_duration_given
      mac 34438249721: missed      linux dafb3cc7c: caught

  runner.rs:82:9: replace TestClock::set with ()
      killed by  src/runner.rs::tests::the_test_clock_can_be_set_forward_to_an_instant
      mac 34438249721: missed      linux dafb3cc7c: caught

  runner.rs:286:9: replace && with || in scan_target_text
      killed by  src/runner.rs::tests::one_half_of_the_exfil_rule_is_not_enough_to_refuse_a_job
      mac 34438249721: missed      linux dafb3cc7c: caught

  runner.rs:359:9: replace JobHandler::dispatch_is_idempotent -> bool with true
      killed by  src/runner.rs::tests::a_handler_that_says_nothing_is_treated_as_not_idempotent
      mac 34438249721: missed      linux dafb3cc7c: caught

  runner.rs:592:9: replace CronRunner::shutdown with ()
      killed by  tests/runner_lifecycle.rs::shutdown_waits_for_a_dispatch_already_in_flight
      mac 34438249721: missed      linux 7f808e952: caught

  runner.rs:606:9: replace <impl Drop for CronRunner>::drop with ()
      killed by  tests/runner_lifecycle.rs::dropping_a_runner_stops_the_task_it_spawned
      mac 34438249721: missed      linux 7f808e952: caught

  runner.rs:1068:31: replace > with >= in drain_published_events
      killed by  tests/event_drain_bounds.rs::an_event_published_the_instant_a_job_was_created_is_consumed_by_it
      mac 34438249721: missed      linux dafb3cc7c: caught

  runner.rs:1086:31: replace < with <= in drain_published_events
      killed by  tests/event_drain_bounds.rs::an_event_exactly_one_interval_after_the_last_fire_is_not_held
      mac 34438249721: missed      linux dafb3cc7c: caught

  runner.rs:1101:20: delete ! in drain_published_events
      killed by  tests/event_drain_bounds.rs::an_event_no_live_subscriber_can_take_is_not_queued_forever
      mac 34438249721: missed      linux dafb3cc7c: caught

  runner.rs:1118:19: replace += with *= in drain_published_events
      killed by  tests/diagnostic_warnings.rs::the_drain_reports_how_many_subscribers_it_actually_fired
      mac 34438249721: missed      linux dafb3cc7c: caught

### store.rs -- 23

  store.rs:35:5: replace default_store_path -> Option<PathBuf> with None
      killed by  tests/store_hardening.rs::the_default_paths_are_real_paths_under_the_wayland_home
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:35:5: replace default_store_path -> Option<PathBuf> with Some(Default::default())
      killed by  tests/store_hardening.rs::the_default_paths_are_real_paths_under_the_wayland_home
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:44:5: replace default_history_path -> Option<PathBuf> with None
      killed by  tests/store_hardening.rs::the_default_paths_are_real_paths_under_the_wayland_home
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:44:5: replace default_history_path -> Option<PathBuf> with Some(Default::default())
      killed by  tests/store_hardening.rs::the_default_paths_are_real_paths_under_the_wayland_home
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:68:9: replace CronStore::list_for_run -> Result<Vec<CronJob>> with Ok(vec![])
      killed by  tests/store_hardening.rs::a_store_without_a_file_runs_everything_it_lists
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:79:9: replace CronStore::cron_dir -> Option<PathBuf> with Some(Default::default())
      killed by  tests/store_hardening.rs::a_store_without_a_file_runs_everything_it_lists
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:145:5: replace set_owner_only_perms with ()
      killed by  tests/store_hardening.rs::a_group_readable_jobs_file_is_tightened_on_load
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:171:19: replace ^= with |= in keyed_hash_hex::fnv1a
      killed by  src/store.rs::tests::the_keyed_hash_folds_each_byte_in_with_xor_not_or
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:219:9: replace FileCronStore::integrity_key -> Option<Vec<u8>> with Some(vec![0])
      killed by  tests/store_hardening.rs::the_integrity_key_is_per_directory_and_persisted_owner_only
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:219:9: replace FileCronStore::integrity_key -> Option<Vec<u8>> with Some(vec![1])
      killed by  tests/store_hardening.rs::the_integrity_key_is_per_directory_and_persisted_owner_only
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:219:9: replace FileCronStore::integrity_key -> Option<Vec<u8>> with Some(vec![])
      killed by  tests/store_hardening.rs::the_integrity_key_is_per_directory_and_persisted_owner_only
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:246:9: replace FileCronStore::check_ownership_and_perms -> Result<()> with Ok(())
      killed by  tests/store_hardening.rs::a_group_readable_jobs_file_is_tightened_on_load
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:251:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in FileCronStore::check_ownership_and_perms
      killed by  src/store.rs::tests::the_ownership_gate_does_not_swallow_a_non_missing_io_error
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:264:46: replace & with ^ in FileCronStore::check_ownership_and_perms
      killed by  tests/store_hardening.rs::an_already_tight_jobs_file_is_left_exactly_as_it_is
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:264:46: replace & with | in FileCronStore::check_ownership_and_perms
      killed by  tests/store_hardening.rs::an_already_tight_jobs_file_is_left_exactly_as_it_is
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:265:17: replace & with ^ in FileCronStore::check_ownership_and_perms
      killed by  tests/store_hardening.rs::an_already_tight_jobs_file_is_left_exactly_as_it_is
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:265:17: replace & with | in FileCronStore::check_ownership_and_perms
      killed by  tests/store_hardening.rs::an_already_tight_jobs_file_is_left_exactly_as_it_is
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:265:25: replace != with == in FileCronStore::check_ownership_and_perms
      killed by  tests/store_hardening.rs::a_group_readable_jobs_file_is_tightened_on_load
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:274:49: replace & with ^ in FileCronStore::check_ownership_and_perms
      killed by  tests/diagnostic_warnings.rs::a_jobs_file_that_was_successfully_tightened_warns_about_nothing
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:274:49: replace & with | in FileCronStore::check_ownership_and_perms
      killed by  tests/diagnostic_warnings.rs::a_jobs_file_that_was_successfully_tightened_warns_about_nothing
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:274:57: replace != with == in FileCronStore::check_ownership_and_perms
      killed by  tests/diagnostic_warnings.rs::a_jobs_file_that_was_successfully_tightened_warns_about_nothing
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:362:23: replace match guard e.kind() == std::io::ErrorKind::NotFound with true in FileCronStore::read_file
      killed by  src/store.rs::tests::a_read_failure_that_is_not_a_missing_file_is_not_reported_as_empty
      mac 34438249721: missed      linux dafb3cc7c: caught

  store.rs:430:20: delete ! in <impl CronStore for FileCronStore>::list_for_run
      killed by  tests/diagnostic_warnings.rs::withholding_untagged_jobs_from_auto_fire_is_announced_once
      mac 34438249721: missed      linux dafb3cc7c: caught

### trigger.rs -- 8

  trigger.rs:285:57: replace < with <= in Trigger::validate
      killed by  tests/trigger_arithmetic.rs::a_webhook_path_must_have_something_after_the_slash
      mac 34438249721: missed      linux dafb3cc7c: caught

  trigger.rs:285:57: replace < with == in Trigger::validate
      killed by  tests/trigger_arithmetic.rs::a_webhook_path_must_have_something_after_the_slash
      mac 34438249721: missed      linux dafb3cc7c: caught

  trigger.rs:332:24: replace > with >= in Trigger::next_after
      killed by  tests/trigger_arithmetic.rs::a_one_shot_due_exactly_now_does_not_re_arm
      mac 34438249721: missed      linux dafb3cc7c: caught

  trigger.rs:339:28: replace + with - in Trigger::next_after
      killed by  tests/trigger_arithmetic.rs::an_interval_projects_forward_by_its_period
      mac 34438249721: missed      linux dafb3cc7c: caught

  trigger.rs:359:28: replace + with - in Trigger::next_after
      killed by  tests/trigger_arithmetic.rs::a_commitment_projects_forward_by_its_heartbeat
      mac 34438249721: missed      linux dafb3cc7c: caught

  trigger.rs:370:24: replace match guard bound.is_spent(t) with false in Trigger::next_after
      killed by  tests/trigger_arithmetic.rs::a_fire_that_lands_past_the_deadline_is_not_scheduled
      mac 34438249721: missed      linux dafb3cc7c: caught

  trigger.rs:378:9: replace Trigger::is_clock_driven -> bool with false
      killed by  tests/trigger_arithmetic.rs::the_clock_driven_variants_say_so
      mac 34438249721: missed      linux dafb3cc7c: caught

  trigger.rs:453:12: replace > with >= in heartbeat_state
      killed by  tests/trigger_arithmetic.rs::a_commitment_is_not_expired_at_its_deadline
      mac 34438249721: missed      linux dafb3cc7c: caught


RECORDED AS NOT WORTH KILLING -- 13 rows
------------------------------------------------------------------------

### EQUIVALENT-release -- 1 row

  lease.rs:365:9: replace ScheduleLease::release with ()
      mac 34438249721: missed      linux dafb3cc7c: missed

  `release` takes `self` BY VALUE, so deleting its body does not stop `self`
  from being dropped when the call returns, and `Drop for ScheduleLease` opens
  with the identical `self.handle.revoke()` before unlocking and removing the
  record. The mutant's entire observable sequence is Drop's, run at the same
  point of the same call. `lease::tests::releasing_lets_the_next_attempt_win`
  already asserts both things `release` promises -- the handle stands down, and
  the schedule is reclaimable -- and it passes against the mutant because the
  mutant genuinely does both. There is no caller that can tell them apart.

### EQUIVALENT-lockflags -- 1 row

  lease.rs:442:64: replace | with ^ in sys::try_lock_exclusive
      mac 34438249721: missed      linux dafb3cc7c: missed

  The operands are `LOCK_EX = 2` and `LOCK_NB = 4`: single, disjoint bits, so
  `2 | 4` and `2 ^ 4` are both `6`. The mutated program issues a bit-for-bit
  identical `flock(2)` call. Nothing distinguishes them because there is nothing
  to distinguish.

### UNREACHABLE-errno -- 1 row

  lease.rs:448:27: replace match guard code == EAGAIN_LINUX || code == EWOULDBLOCK_BSD with true in sys::try_lock_exclusive
      mac 34438249721: missed      linux dafb3cc7c: missed

  Widening the guard to `true` routes every `flock` errno to `Ok(false)` instead
  of `Err`, so it is observable only if `flock(2)` can hand this module an errno
  other than `EAGAIN`/`EWOULDBLOCK`. It cannot. The descriptor is one this module
  opened itself, on a regular one-byte sentinel file it created in a directory it
  created, and the operation is the constant `LOCK_EX | LOCK_NB`. That leaves
  `EBADF` and `EINVAL`, both impossible by construction, and `EWOULDBLOCK`, which
  the guard already matches. The `Err` arm is defensive rather than reachable, so
  there is no input a test could supply to enter it. (The `None` arm below it is
  unreachable for a second reason: `std::io::Error::last_os_error()` always
  carries a `raw_os_error`.)

### PLATFORM-eagain -- 1 row

  lease.rs:448:32: replace == with != in sys::try_lock_exclusive
      mac 34438249721: missed      linux dafb3cc7c: caught

  MEASURED, not argued. The guard is
  `code == EAGAIN_LINUX || code == EWOULDBLOCK_BSD`, and those constants are 11
  and 35. On Linux a contended `flock` returns 11, so negating the first
  comparison makes the guard `11 != 11 || 11 == 35` false and a second attempt
  returns `Err` where it must return `Observer` -- and on the Linux run below
  this mutant is CAUGHT, by `lease::tests::a_second_attempt_in_one_process_is_refused`.
  On macOS/BSD, which is where the reporting run ran, a contended `flock` returns
  35, the first comparison is false with or without the mutation, and the mutant
  is exactly equivalent. No test can kill it there because there is nothing there
  to kill; the test that kills it already exists and already runs.

  The mirror row is the control that makes this a measurement rather than a
  story: `lease.rs:448:56` (`== EWOULDBLOCK_BSD` -> `!=`) is CAUGHT on macOS and
  MISSED on Linux -- the exact opposite -- by the same test, for the same reason
  with the constants swapped.

### CFG-WINDOWS -- 7 rows

  lease.rs:503:9: replace sys::try_lock_exclusive -> std::io::Result<bool> with Ok(false)
      mac 34438249721: missed      linux dafb3cc7c: missed
  lease.rs:503:9: replace sys::try_lock_exclusive -> std::io::Result<bool> with Ok(true)
      mac 34438249721: missed      linux dafb3cc7c: missed
  lease.rs:521:41: replace | with & in sys::try_lock_exclusive
      mac 34438249721: missed      linux dafb3cc7c: missed
  lease.rs:521:41: replace | with ^ in sys::try_lock_exclusive
      mac 34438249721: missed      linux dafb3cc7c: missed
  lease.rs:528:15: replace != with == in sys::try_lock_exclusive
      mac 34438249721: missed      linux dafb3cc7c: missed
  lease.rs:533:13: delete match arm Some(ERROR_LOCK_VIOLATION) | Some(ERROR_IO_PENDING) in sys::try_lock_exclusive
      mac 34438249721: missed      linux dafb3cc7c: missed
  lease.rs:540:9: replace sys::unlock with ()
      mac 34438249721: missed      linux dafb3cc7c: missed

  Inside `#[cfg(windows)] mod sys` (lease.rs:461-555). The reporting run was
  hosted `macos-latest`, where that block is not compiled at all, so the mutation
  could not change the binary the tests ran against. The MISSED status is a
  property of the host's `cfg` selection, not of the test suite, and no test
  written or run on a Unix host can move it. Control: the same seven rows are
  MISSED on the Linux run below, on a tree whose lease tests were strengthened,
  for the same reason -- which is what a host artifact looks like and is not what
  a test gap looks like.

  Grading them needs a Windows-hosted mutation leg. The nightly matrix has none
  (every job is `runs-on: macos-latest`), so this is a workflow gap belonging to
  core#424's matrix rather than a wcore-cron test gap, and it is recorded here as
  such rather than being counted as either killed or excused.

  UNPROVEN, stated as a prediction and not as a pass: the lease suite is not
  `cfg`-gated, so `a_second_attempt_in_one_process_is_refused` and
  `releasing_lets_the_next_attempt_win` do drive `sys::try_lock_exclusive` on
  Windows. On that host the `Ok(true)` and `Ok(false)` rows at 503:9 would break
  the observer half and the owner half of those assertions respectively. Nothing
  here demonstrates it.

### CFG-FALLBACK -- 2 rows

  lease.rs:564:9: replace sys::try_lock_exclusive -> std::io::Result<bool> with Ok(false)
      mac 34438249721: missed      linux dafb3cc7c: missed
  lease.rs:564:9: replace sys::try_lock_exclusive -> std::io::Result<bool> with Ok(true)
      mac 34438249721: missed      linux dafb3cc7c: missed

  Inside `#[cfg(not(any(unix, windows)))] mod sys` (lease.rs:556-578), the
  unsupported-platform stub. It is not compiled on ANY target this repository
  builds or ships -- the CI matrix is macOS, Linux and Windows -- so no mutation
  run on any host can grade it and no test on any host can reach it. This is the
  one disposition in this file that is already final: the stub exists so the
  crate still compiles if someone points it at an exotic target, and until such a
  target is supported there is nothing there to test.


PER-FILE TOTALS
------------------------------------------------------------------------

  | file | reported missed | killed | recorded |
  |---|---|---|---|
  | `events.rs` | 1 | 1 | 0 |
  | `history.rs` | 1 | 1 | 0 |
  | `lease.rs` | 19 | 6 | 13 |
  | `retry.rs` | 1 | 1 | 0 |
  | `runner.rs` | 11 | 11 | 0 |
  | `store.rs` | 23 | 23 | 0 |
  | `trigger.rs` | 8 | 8 | 0 |
  | **total** | **64** | **51** | **13** |

