---
issue: 449
repo: FerroxLabs/wayland-core
kind: defect
title: "[mutants-nightly] wcore-cron — surviving mutants (2026-09-04)"
status: open
last_verified_commit: 375efb70b
criteria:
  - id: c1
    text: "The surviving mutants reported for wcore-cron are dispositioned: each is either killed by a new or strengthened test, or recorded with a reason it is not worth killing."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, and it is 64 rows short rather than 44: the issue body shows only the TOP 20 of the 64 survivors, so nothing filed on this ticket has ever named the other 44. The full inventory is now recorded in the prose below, read from artifact mutants-log-wcore-cron of run 33844721279 (mutants.out/missed.txt, exactly 64 lines, artifact id 9932557850, downloaded 2026-09-10 while still unexpired) -- not from the issue body and not from the job log. WHAT THE FULL LIST CHANGES: the 20 shown are 19 lease.rs plus 1 events.rs, which reads as `this is a file-locking problem`. Over all 64 the distribution is store.rs 23, lease.rs 19, runner.rs 11, trigger.rs 8, events.rs 1, history.rs 1, retry.rs 1 -- so the LARGEST untested cluster is `store.rs`, and 11 of those 23 sit in `FileCronStore::check_ownership_and_perms` and `integrity_key`, i.e. the permission and integrity checks, none of which the issue body ever displayed. THREE ROWS ARE NOT TEST GAPS AT ALL, and this is a classification, NOT a claim that anything is killed: lease.rs gates its backends at `#[cfg(unix)]` (line 417), `#[cfg(windows)]` (461) and `#[cfg(not(any(unix, windows)))]` (556), and this run was hosted `macos-latest`. The 7 mutants at lease.rs:503-540 are inside the windows block and the 2 at lease.rs:564 are inside the never-built fallback, so on this host they were compiled out and could not have failed a test. They are UNGRADED, not equivalent and not caught -- the windows 7 need a Windows mutation leg to say anything about, and the fallback 2 are unreachable on every platform this repo builds, so no nightly can ever grade them. The remaining 55 were live on the host that ran them and are genuine survivors. STILL NOT MET because none of the 55 has been killed. Killing them means new or strengthened tests under crates/wcore-cron/, which this lane does not own and did not touch; this entry buys the next worker a complete, classified starting set of 55 instead of an excerpt of 20. Reported figures reconcile: 339 = 64 missed + 196 caught + 77 unviable + 2 timeouts, catch rate 196/(196+64) = 75.4%. The 2 timeouts are runner.rs:202 and runner.rs:239, both `replace + with -`, and are recorded below as neither caught nor missed. TESTS ARE NOW WRITTEN AND c1 STILL STANDS not-met, BECAUSE NOTHING HAS RUN THEM (375efb70b, 2026-09-10). 29 of the 55 live survivors now have a test written to fail against them, each named by file and line in the test`s own doc comment: 19 of the 23 in store.rs, all 8 in trigger.rs, events.rs:202:5 and retry.rs:71:25. WHERE: crates/wcore-cron/tests/store_hardening.rs (the public-API arms), crates/wcore-cron/tests/trigger_arithmetic.rs, and three tests appended to the inline cfg(test) module of crates/wcore-cron/src/store.rs for the mutants that sit behind private items. No production line moved -- every mutant line number listed above still resolves to the source line it named, checked one by one after the edit. THE store.rs 19: 35:5 x2 and 44:5 x2 (the default paths), 68:9 and 79:9 (the two CronStore trait defaults, which only a store with no file reaches, so the test carries its own in-memory store), 145:5 (set_owner_only_perms), 171:19 (the XOR fold in the keyed hash), 219:9 x3 (integrity_key), 246:9 (the whole ownership gate replaced by Ok(())), 251:23 and 362:23 (the two NotFound match guards), and 264:46 x2, 265:17 x2 and 265:25 (the mode arithmetic). THE FOUR store.rs ROWS NOT TARGETED, STATED AS A LIMIT AND NOT AS A PASS: 274:49 x2, 274:57 and 430:20 change only whether a tracing::warn! fires, and nothing in this crate captures tracing output, so no in-tree test can observe them without a subscriber harness and a dev-dependency this lane did not add. The uid branch of the gate (meta.uid() != uid) needs a file owned by a SECOND user, which a single-uid test process cannot create; it needs a privileged fixture, not a better test. lease.rs (10), runner.rs (11) and history.rs (1) are untouched. history.rs:122:19 is the same NotFound-guard shape as the two graded here and has the same recipe: call the private reader directly on a path one of whose directory components is a regular file, so the error is ENOTDIR rather than ENOENT, with a genuinely-missing path as the control in the same test. TWO DESIGN POINTS THAT ARE THE DIFFERENCE BETWEEN A TEST AND A KILL. (1) The permission fixture is 0400, NOT 0600. Five of the survivors (264:46 x2, 265:17 x2, 265:25) make the loose-mode test true for a file that is ALREADY tight, and the branch they then wrongly enter calls set_owner_only_perms, which writes 0600 -- so against a 0600 fixture the spurious tighten is a no-op and completely invisible. At 0400 the mode is the witness. Each of those tests asserts the chmod took before asserting what the load did, so a filesystem that ignores modes reds as a control failure rather than passing vacuously. (2) The trigger tests use TriggerBound::new(1, 1) rather than each variant`s default_bound(), because every default floors the interval at 60s and the result floor -- not the arithmetic under test -- then decides the answer. That is precisely why those eight survived a suite which already exercises next_after. VERIFICATION OWED, AND NOT PERFORMED. (a) `cargo nextest run --locked -p wcore-cron --retries 0` green on Linux and on macOS. These tests have never been compiled, let alone run: no build slot was free and cargo may not be run on the build Mac. `cargo fmt --all` exits 0, which proves rustfmt PARSES all three files and proves nothing about types, names, or outcomes. (b) THE ACTUAL KILL, which is the only thing that can move c1: re-run cargo-mutants over wcore-cron on the same host and confirm each of the 29 named rows has moved from missed to caught. A test that compiles and passes is not a kill; the mutant has to be shown to die, and a mutation test that cannot fail grades nothing. Until (b) reads back, the honest state of the 29 is `a kill is claimed and not yet demonstrated` and of the other 26 `still open`, and c1 is not-met for both halves. GREEN ARM ADDED at bd3d32d00: the tests the previous pass could not compile now BUILD AND PASS on the Linux proof host -- cargo test --locked -p wcore-cron is 0 failures across every target, and the two new files are store_hardening 7/7 and trigger_arithmetic 7/7. That upgrades them from 'written, never executed' to a real green arm, and it retires the previous note's caveat that only rustfmt had parsed them. IT DOES NOT CLOSE c1: a passing test is not a KILL. What is still owed is unchanged -- re-run cargo-mutants over wcore-cron and confirm each of the 29 named rows moved missed -> caught. A test that passes against unmutated code says nothing about whether it fails against the mutant it was written for."
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
