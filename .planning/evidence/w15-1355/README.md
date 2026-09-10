# wayland#1355: the session index lock's 1 s give-up, and the ENOENT beside it

Host `hetzner-dsm` (Linux 6.8.0-101-generic, 96 CPU, 251 GiB). Builds and
test runs go through `tools/remote-proof.py` slot `default`. Every receipt below
has `"complete": true` and the stated `remote_exit`. Load loops ran under
`nohup` on the same host from COPIES of the test binaries, identified by
sha256 and checked by content (below). Every iteration is one libtest process
(`cargo test` semantics: one shared process, no retries).

## Commits

| commit | branch | what | merge? |
|---|---|---|---|
| `9cd1e5a2d` | `w15/idxlock1355` | tests + `cfg(test)` seams only, lock unchanged | yes |
| `e1c3bf704` | `w15/idxlock1355` | the repair | yes |
| `0d0edc54c` | `w15/idxlock1355` | review follow-up: sentinel released on unwind, kill-on-drop test child, doc corrections | yes |
| `516a6b3c2` | `w15/idxlock1355-red-c2` | RED ARM: fix tree with the in-process slot removed | NEVER |
| `439d54417` | `w15/idxlock1355-red-harness` | RED ARM: fix tree with the old unwinding join shape | NEVER |

`crates/wcore-agent/src/session.rs` is the only source file touched. No
contract `SOURCE_INPUTS` file (engine.rs, bootstrap.rs) was edited.

## c1: the two errors, and the instrument that named each

CI evidence: `ci-diagnostics-linux-f4563ffe6`, `shared-lib/stdout.log`, run
34476606238, `test result: FAILED. 2732 passed; 1 failed ... 81.27s`.

    2945  thread '<unnamed>' (2644) panicked at session.rs:1624:55:
    2946  ... Could not acquire index lock after 1s
    2948  thread '<unnamed>' (2643) panicked at session.rs:1624:55:
    2949  ... Could not acquire index lock after 1s
    2951  thread 'session::tests::test_f033_index_lock_parallel' (2637) panicked at session.rs:1630:22:
    2954  thread '<unnamed>' (2648) panicked at session.rs:1624:55:
    2955  ... No such file or directory (os error 2)

**Cause 1, the 1 s give-up.** Named by CI's own panic text and by the code at
base `5b7c18412`, `session.rs:1016`. `acquire_sentinel_lock` spins in 10 ms
steps to a fixed 1 s deadline, and it applied that deadline to writers in the
SAME process. Their hold includes `atomic_write`'s `sync_all` of `index.json`,
so ten queued writers under CI's CPU and fsync latency push the last waiter
past 1 s. Deterministic red test
`test_1355_writer_waits_out_an_in_process_hold_past_one_second`. It releases
the holder only once the waiter has been SEEN contending 1.5 s into its acquire
(a `cfg(test)` contention recorder), and the old loop bails on its first
contended look past 1 s, so no load can turn it green.

- RED at `9cd1e5a2d`: `remote_exit 101`, `5 passed; 1 failed`, the failure is
  exactly `Could not acquire index lock after 1s` (status sha256 `6aa4e42a`).

**Cause 2, the ENOENT: a teardown artifact of the test, not a lock race.**
Named by the ORDER of libtest's shared capture buffer. Spawned threads inherit
the test's capture, and each panic message is appended when it happens. The
order is: two give-ups, then the TEST thread's panic at `h.join().unwrap()`
(`:1630`), then the ENOENT. The old harness joined in spawn order with
`unwrap`, so the first failed writer unwound the test thread while later
writers were still running. Unwinding dropped the `TempDir` (a recursive
delete). A writer parked in `acquire_sentinel_lock` then called `create_new` in
a directory that no longer existed, and got `NotFound`, which the loop returns
raw (`Err(e) => return Err(e.into())`).

The cause was settled by exhaustion. Every ENOENT-producing step of
`persist_first_message` needs a missing parent: the sentinel's `create_new`,
`NamedTempFile::new_in`, and the `persist` rename. Nothing else knows the
private tempdir. The lock-lifecycle races (a release by path, a stale steal)
cannot produce ENOENT; they produce a second holder, which the new holder
gauge measures.

- Mechanism reproduced deterministically:
  `test_1355_a_parked_writer_reports_enoent_when_its_store_is_removed`. Its
  result is `NotFound`, raw os error 2 on unix. It is green on every arm,
  because it demonstrates the mechanism rather than the repair.

NOT CLAIMED for c1: why CI's holds were that long. On this host the OLD binary's
worst measured in-process wait was 516,941 us (below); CI's latency was not
reproduced here.

## c2: the repair, and the proof that exclusion held

Repair (`e1c3bf704`): writers in this process share a per-canonical-directory
in-process slot (`InProcessIndexLock`). The slot is taken BEFORE the sentinel
and released only AFTER the sentinel has been removed. The wait is bounded by
the same 30 s the sentinel treats as proof of a dead holder; a waiter that
reaches the bound errors and never steals. Only the slot's holder touches the
sentinel, so no in-process writer can meet, steal or delete another's sentinel.
The sentinel's own rules (1 s budget, 30 s stale steal) are unchanged, but the
slot changes cross-process fairness and caller blocking; see "Trades" below.
The slot is not a FIFO queue.

Deterministic arms of `test_1355_writer_waits_out_an_in_process_hold_past_one_second`:

| source | result | receipt |
|---|---|---|
| `9cd1e5a2d` (no fix) | RED, `Could not acquire index lock after 1s` | remote_exit 101, status `6aa4e42a` |
| `516a6b3c2` (fix minus the slot) | RED, same message | remote_exit 101, status `7cd7b4cc` |
| `e1c3bf704` (fix) | green | remote_exit 0, status `14f2bc53` |

At `e1c3bf704` the full `cargo test -p wcore-agent --lib` passed 2744, failed 0
and ignored 3, all in one process (24.72 s). `cargo clippy -p wcore-agent
--all-targets -- -D warnings` returned remote_exit 0.

Also at `e1c3bf704`:

- `test_1355_a_killed_writers_index_lock_is_recovered`: a real child process
  (this test binary, re-entered) takes the lock and is killed inside it. The
  test asserts the sentinel left behind holds the child's pid, moves only the
  sentinel's mtime past 30 s, and recovers.
- `test_1355_a_live_foreign_holders_sentinel_is_never_stolen`: a fresh sentinel
  holding a foreign pid is left byte-identical, and `index.json` is never
  written.
- `test_f033_index_lock_parallel` now asserts a holder-gauge maximum of exactly
  1, and that the listed ids equal the committed ids.

Binaries: old `wcore_agent-9cd1e5a2d` sha256 `332474bb...`, fix
`wcore_agent-e1c3bf704` sha256 `4ffa97a0...`. Content check: the fix's error
string `another writer in this process still holds it` occurs 1 time in the fix
binary and 0 times in the old one; the control string `Could not acquire index
lock after 1s` occurs once in each.

Load arms. The load was pinned busy loops plus 2 `O_DSYNC` `dd` writers on the
same CPUs as the test, and each iteration was one test process.

| binary | load | test args | n | result | worst wait / hold |
|---|---|---|---|---|---|
| fix | CPU 95, 32 burners | `--exact ...test_f033_index_lock_parallel` | 200 | **200/200**, each `1 passed`, 10 acquisitions and 10 writes every run | 663,502 us / 210,720 us |
| fix | CPU 94-95, 12 burners, `--test-threads 16` | `session::` (40 tests in one process) | 100 | **100/100**, each `40 passed; 0 failed`, 10 acquisitions and 10 writes every run | 97,515 us / 70,367 us |
| old | CPU 95, 32 burners | `--exact ...test_f033_index_lock_parallel` | 50 | 50/50 | 516,941 us / 242,837 us |
| old | CPU 94-95, 12 burners | same | 50 | 50/50 | 474,353 us / 90,894 us |
| old | CPU 94-95, 12 burners, `--test-threads 16` | `session::`, budget test skipped | 10 | 10/10 | |
| old | CPU 94-95, 12 burners, `--test-threads 16` | full lib, budget test skipped | 5 | `test_f033_index_lock_parallel ... ok` 5/5; every process then aborted, see below | |

**What the load arms do NOT show.** They do not DISCRIMINATE: the old binary
also passed every loaded iteration, and no measured wait on this host reached
1 s. They show that exclusion and entry integrity hold under load at
`--retries 0` on the repaired code. The deterministic test and its two red arms
are what show the give-up is gone.

Unrelated finding from the full-lib arm: under 2 pinned CPUs and
`--test-threads 16`, every one of the 5 processes aborted with `thread
'spawner::production_durable_spawn_tests::concurrent_near_cap_admits_exactly_one_retained_workspace'
has overflowed its stack`. It is not #1355's, it was not investigated, and no
ticket was filed.

## c3: the ENOENT

The ENOENT CI observed was the harness. Repaired by `join_every_writer`: every
writer is joined before any is judged, so the store outlives all of them and
each failure reports its own cause. Red test
`test_1355_a_failed_writer_does_not_remove_the_store_under_a_parked_sibling`
injects a failing writer while a sibling is parked on the lock.

- RED on `439d54417` (the fix tree with the old join shape):
  `remote_exit 101`, `5 passed; 1 failed`, panicked at `session.rs:1422` on
  the injected failure (status `ae6fadd8`).
- Green at `e1c3bf704`.

NOT CLAIMED, and it is a correction to the ticket: **the ENOENT class is NOT
unreachable in production.** `wayland-core backup restore --replace` clears the
target home, `sessions/` included, with `remove_dir_all`
(`crates/wcore-cli/src/backup/restore.rs:234` `clear_target`; the journal
rollback does the same at `backup/journal.rs:722`). It checks for a live
RESTORE owner, but not for a live engine writing that home. An engine in
another process that is parked on, or inside, the index lock at that moment gets
the same bare `No such file or directory (os error 2)`. Failing is the correct
outcome there, since its store is being replaced, but the message does not name
the path. This lane did not change it.

## c4

The `.config/flaky-allowlist.txt` row for `test_f033_index_lock_parallel`
(line 63, expiry 2026-09-15, gh#1169) is deleted, not renewed. The
shared-process lib step honours no allowlist, so the row never protected the
check it reddened.

## Trades the repair makes (disclosed after review of `e1c3bf704`)

- The in-process slot is not FIFO. A writer arriving at a release can take it
  ahead of the waiter that release woke, and waiters that were not woken
  re-check every 50 ms. What is guaranteed is exclusion and the 30 s bound.
- Cross-process fairness got worse. The next in-process writer takes the
  sentinel within microseconds of its release, while a writer in another
  process polls every 10 ms on a 1 s budget. A busy process can starve another
  process's writer, and `engine.rs:22165` then logs and drops that update.
- Callers block longer, deliberately. `persist_first_message`
  (`engine.rs:14160`, inside `async fn run_inner_impl`) and `update_index_for`
  (`engine.rs:22165`) run synchronously on async-runtime worker threads. Under
  an fsync stall a waiter now blocks for up to about 30 s instead of failing
  after 1 s: a durable index write waits for the disk.
- Against a fresh foreign sentinel, in-process waiters now fail one after
  another, the k-th after about k seconds and at most 30 s, instead of all of
  them after about 1 s.

## Residuals, stated

- Writers in DIFFERENT processes still get the sentinel's fixed 1 s budget;
  `engine.rs:22165` still logs and drops the index update when that runs out.
- Between processes, the stale steal (two waiters judging one stale sentinel)
  and release by path after a hold over 30 s can still admit two holders.
  Unchanged, and pre-existing.
- Changed at `0d0edc54c` after review: a panic inside the index closure used to
  leave this process's own sentinel for 30 s while releasing the in-process
  slot, so every later writer here failed on its 1 s budget. Release is now a
  `Drop` guard: sentinel first, slot second, on every path. Test
  `test_1355_a_panic_inside_the_index_lock_releases_the_sentinel`. RECEIPTS
  PENDING a build slot.
- `with_wal_lock` uses the same sentinel and the same 1 s in-process budget,
  and was not touched.
- Not run on Windows or macOS; no `cfg(windows)` code changed.
