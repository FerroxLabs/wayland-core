# Workspace-wide sweep for the fork-duplicated `flock` defect

Lane: lock-sweep. Branch `lane/lock-sweep`, worktree `/root/wt-locksweep`, base
`8d69d402` (`integ/round2-base`). All builds and runs on `hetzner-dsm`.

Follow-on to `.planning/LIB-NONDETERMINISM-NOTES.md` §7 item 1 ("HIGH — audit
every other `flock` holder for the same assumption") and item 2 (the snapshot
locks). Read that document first; the mechanism is not restated here.

**Headline: three further vulnerable holders, all fixed. The worst is not in
the agent — it is `wcore-gateway`'s `PidLock`, whose sentinel is a stable
pathname, so a pinned lock makes a STOPPED gateway refuse the next `gateway
start` with `AlreadyHeld` naming a pid that is gone.**

---

## 1. The sweep

Every OS-level file lock in the workspace, found by grepping `crates/` for
`flock`, `FlockArg`, `lock_exclusive`, `lock_shared`, `try_lock`, `LOCK_EX`,
`LOCK_UN`, `LOCK_SH`, `LockFileEx`, `UnlockFileEx`, `fd_lock`, `libc::flock`
and `File::try_lock`, then cross-referencing each hit against its `Drop`. The
lock-bearing dependency surface is exactly one crate — `fd-lock 4.0.4`, declared
in `wcore-budget`, `wcore-channels`, `wcore-config` and `wcore-swarm`. There is
no `fs2`, `file-lock`, `named-lock` or `advisory-lock` anywhere in the tree.

### Raw `flock` / `LockFileEx` holders

| # | Holder | Verdict | Why |
|---|---|---|---|
| 1 | `wcore-agent/src/session_journal/lease.rs` — `WriterLease` (`.writer.lock`) | SAFE | `WriterLease::drop` calls `unlock_authority` explicitly. This is the reference pattern; it is why the sentinel had 0 hits in the previous lane's instrumentation. |
| 2 | `wcore-agent/src/session_journal/lease.rs::lock_data_file` — journal data inode, held by `JournalWriter` and `SessionStorageLease` | SAFE | Fixed by the previous lane (`02833d6b`): `lease::unlock_data_file` + both `Drop` impls. |
| 3 | `wcore-agent/src/session_journal/lease.rs::read_owner` (`:726`) — liveness probe on the sentinel | SAFE | On `Acquired` it calls `unlock_authority` before returning; on `Contended` no lock was taken. The probe never leaves one behind. |
| 4 | `wcore-agent/src/session_journal/snapshot.rs::replace_file_atomically_inner` (`:1233`) — the `.snapshot` file, the `.authority` head, and the compacted journal | **VULNERABLE → FIXED** | Returned a locked `File` that four call paths dropped without `LOCK_UN`, across eight distinct early-return edges. See §2.2. |
| 5 | `wcore-cron/src/lease.rs` — `ScheduleLease` (and `wcore-agent/src/channel_lease.rs`'s `ChannelPollLease`, which reuses it) | SAFE | Fixed by the previous lane (`ee3fbe02`). |
| 6 | `wcore-gateway/src/pidlock.rs` — `PidLock` (`gateway.lock`) | **VULNERABLE → FIXED** | `Drop` removed the status record and stated "The OS releases the lock itself when `_sentinel` closes." See §2.1. |
| 7 | `wcore-eval-scenarios/src/process_tree.rs` — `Cgroup::_identity_lock` (`/run/wayland-eval-identity-<uid>-<gid>.lock`) | **VULNERABLE → FIXED** | A bare `File` field with no unlock anywhere, held across a forking candidate process tree. See §2.3. |

### `fd_lock::RwLock` holders

All SAFE, and safe *by construction* rather than by inspection:
`fd-lock`'s `RwLockWriteGuard::drop` calls `flock(fd, LOCK_UN)` on Unix
(`src/sys/unix/write_guard.rs`) and `UnlockFile` on Windows
(`src/sys/windows/write_guard.rs`). Every site below binds the guard to a
`let` for the critical section and lets it drop; none is `mem::forget`ed.

| Holder | Shape |
|---|---|
| `wcore-budget/src/daily.rs::with_locked_ledger` | `_guard` held across load → body → store |
| `wcore-channels/src/dispatch/pairing.rs::update` | `_guard` held across load → body → save |
| `wcore-config/src/credentials.rs::with_marker_lock` | `_guard` held across `body()` |
| `wcore-config/src/confidential_blob.rs` (`:146`) | `_guard` held across key load-or-create |
| `wcore-swarm/src/worktree.rs::with_directory_lock` (`:420`) | `_guard` held across `action()` |
| `wcore-swarm/src/worktree.rs::transaction_is_active` (`:526`) | `try_write` probe; the guard drops at the end of the match arm |
| `wcore-swarm/src/worktree.rs` `ActiveLease` (`:143`) | guard owned by a dedicated thread, released by an explicit `drop(guard)` on the channel signal |
| `wcore-swarm/src/worktree/parent.rs:321` | landing lock, `_lock_guard` held across CAS + projection |
| `wcore-swarm/src/worktree/parent.rs:465` | rollback landing lock, same shape |

### Lockfile holders — a different mechanism, not this defect

These take no OS lock at all. Both create the lockfile with `O_CREAT|O_EXCL`,
**close the handle immediately** after stamping it, and release by unlinking.
There is no open file description to duplicate, so `fork(2)` cannot pin them.

| Holder | Verdict |
|---|---|
| `wcore-config/src/credentials.rs` — `ExclusiveFileLock` | SAFE. Backs the OAuth refresh lock (`wcore-agent/src/oauth/refresh_lock.rs`), the credential-store write lock, and `.credentials.migrate.lock`. Its liveness problem is a *stale holder*, which is why it carries a nonce and a heartbeat — an orthogonal design, correctly chosen. |
| `wcore-agent/src/orchestration/anvil/lease.rs` — `ClimbLease` | SAFE. Same shape, plus a pid compare so a successor's lease is never clobbered. |

Both were named in the previous lane's follow-up list as unswept. Neither has
this defect, and neither should be converted to `flock` to get one.

### Matched by the grep, not file locks

`wcore-cli/src/tui/engine_bridge.rs`, `wcore-mcp/src/transport/stdio.rs`,
`wcore-skills/src/refs.rs`, `wcore-swarm/src/lib.rs` (`dispatch_gate`) and
`wcore-tools/src/file_state.rs` all match `try_lock` / `ManuallyDrop` but are
in-process `Mutex`es. The `mem::forget(guard)` sites in `wcore-cli/src/backup/`
and `wcore-cli/src/migrate/` are tests deliberately simulating SIGKILL against
journal markers, not locks.

---

## 2. What was fixed

### 2.1 `wcore-gateway` `PidLock` — the worst of the three

`Drop for PidLock` removed the status record and left the lock to `close(2)`,
with the wrong assumption stated in the comment. The gateway hosts an agent
that spawns subprocesses constantly, and unlike the session journal its
sentinel is a **stable pathname and a stable inode**: the next `gateway start`
locks the same inode. So this is not a descriptor leak, it is an availability
failure — a gateway that has stopped refuses the next launch, and the refusal
names a pid that no longer exists (`AlreadyHeld { pid: 0 }` once the record has
been removed, which `Drop` does first).

Fix: `unlock()` for unix (`flock LOCK_UN`) and windows (`UnlockFileEx` over the
same one-byte range the claim covers), called from `Drop`. `_sentinel` renamed
to `sentinel` because it is now read.

**Red arm** — fix reverted, source `touch`ed, rebuilt:

```
test a_released_home_is_reclaimable_while_a_forked_child_never_execs ... FAILED
  releasing a gateway pid lock must free the home even while a forked child
  still holds the open file description: Some(AlreadyHeld { pid: 0 })
test releasing_the_lock_removes_the_kernel_record_despite_a_forked_child ... FAILED
  a released pid lock must leave no lock in the kernel; ... 1 record(s) survived:
  ["2: FLOCK  ADVISORY  WRITE 2320933 09:02:85219881 0 EOF"]
test result: FAILED. 0 passed; 2 failed ... finished in 0.02s
```

Deterministic in 0.02 s. The second case grades `/proc/locks` — the kernel's
own record — not the API's return value, and carries a positive control that
fails the test if a HELD lock is not visible to the sampler. That control
caught a real bug in my first `/proc/locks` parser.

Green arm: 5/5 runs, 2 passed each, 0.01–0.02 s.
Probe: `crates/wcore-gateway/tests/pidlock_fork_pin.rs`.

### 2.2 The snapshot publication path

`replace_file_atomically_inner` locks every replacement inode before
publication and hands the locked handle back. Four paths consumed it:

* `write_snapshot_authority_head` discarded it outright (`replace_private_file_atomically(..)?;`)
* `compact`, `publish_snapshot` and `reconcile_snapshot_authority_head` each
  held it and ended with a bare `drop(snapshot_file)`
* `compact` additionally overwrote `self.file` with the new journal handle,
  dropping the outgoing one unlocked

Between them those four sites have **eight** early-return edges. A release that
has to be remembered on each of them is a release that gets forgotten — which
is how the leak arrived. So the fix here is a guard, `snapshot::LockedFile`,
not eight `unlock_data_file` calls: `Deref`/`DerefMut` to `File`, `Drop`
issuing `LOCK_UN`, and one `into_locked_inner()` escape hatch used exactly
once, where `compact` transfers the incoming handle to `JournalWriter` (whose
own `Drop` then owns its release). The two deliberate `std::mem::forget`
fail-closed leaks inside `replace_file_atomically_inner` stay on the raw
`File`, outside the guard, so their fail-closed semantics are unchanged.

The public `session_journal::write_snapshot` wrapper's explicit
`unlock_data_file` (added by the previous lane) is now redundant and was
folded into the guard.

**Red arm** — both source files reverted, `touch`ed, rebuilt, 5 runs:

| run | result | published files still locked |
|---|---|---|
| 1 | FAILED | 43 of 72 |
| 2 | FAILED | 46 of 72 |
| 3 | FAILED | 38 of 72 |
| 4 | FAILED | 37 of 72 |
| 5 | FAILED | 41 of 72 |

with records such as
`199: FLOCK ADVISORY WRITE 2455658 09:02:84978084 0 EOF` against
`probe-0.journal.snapshot`. The positive control passed in all five.

Green arm: 5/5 runs, 2 passed, **0 of 72**, 0.16–0.18 s.
Probe: `crates/wcore-agent/tests/snapshot_lock_probe.rs`, production APIs only
(`SessionJournal::open` / `publish_snapshot` / `compact`), grading
`/proc/locks`.

The probe samples three files per iteration — the journal, its `.snapshot` and
its `.authority` head — 72 files in all, so it also carries a built-in control
that the sampler is not simply flagging everything. Every red-arm count is at
or below 48, the ceiling reachable if only the two snapshot companions can
leak, and no `.journal` path appears in any of the samples the assertion
printed. That is consistent with the previous lane's journal fix holding; it
is weaker than an exhaustive per-suffix breakdown, which I did not take,
because re-running the mutated arm would have rebuilt the library out from
under the five suite runs in flight.

**On severity.** On Unix this one is a lock leak with no reachable *refusal*:
each publication replaces the inode, so nothing re-locks a published snapshot,
and advisory locks do not block readers. On Windows `LockFileEx` is
**mandatory** over its range, so a pinned lock blocks `load_snapshot` and
`load_snapshot_authority_head` from reading the very files recovery depends on.
That arm is unverifiable from here; the Windows clippy gate proves only that it
compiles.

### 2.3 `wcore-eval-scenarios` candidate identity lock

`Cgroup` held the identity claim in a bare `_identity_lock: File`. The evaluator
forks the candidate process tree while it is held, and the path is stable
(`/run/wayland-eval-identity-<uid>-<gid>.lock`), so a pinned lock strands the
next evaluator for the full 30 s wait and then fails it with "candidate
identity remained assigned to another evaluator" against an identity nobody is
using. The pre-fix code also leaked the lock on the `ensure_identity_inactive`
error path.

Fix: `IdentityLock`, a guard whose `Drop` issues `LOCK_UN`. The acquisition
loop moved unchanged into `IdentityLock::claim` so the release contract can be
exercised without root, `/run` or a delegated cgroup — none of which the
release contract depends on.

**Red arm** — `LOCK_UN` removed from `Drop`, source `touch`ed, rebuilt:

```
releasing an identity must leave no lock in the kernel; ... 1 record(s)
survived ...: ["25: FLOCK  ADVISORY  WRITE 2537773 09:02:85356467 0 EOF"]
test result: FAILED. 0 passed; 1 failed ... finished in 0.01s
```

Deterministic in 0.01 s, positive control included. Green arm passes.
Test: `process_tree::linux::identity_lock_release`.

---

## 3. What this sweep does NOT establish

* **The Windows arms are compiled, not run.** `cargo clippy --target
  x86_64-pc-windows-gnu` passes for all three changed crates, which proves the
  `UnlockFileEx` branch and the `cfg(windows)` code type-check. It proves
  nothing about mandatory-lock behaviour, which is where the snapshot leak is
  most likely to have a user-visible consequence.
* **No filed user report is attributed to any of these.** As with the previous
  lane, the defects are real and proven reachable; that is a different claim
  from "this explains a bug on file". The gateway one is the most likely
  candidate for a future report, because its symptom ("gateway already running"
  when it is not) is user-visible and self-inflicted.
* **The `fd-lock` verdicts rest on `fd-lock 4.0.4`'s guard `Drop`.** They are
  correct for the pinned version and were read from the vendored source, not
  assumed from the crate's documentation. A major-version bump should re-check
  `sys/*/write_guard.rs`.

---

## 4. Gates and the five-run suite

On the committed tree (`43380205`), all four gates clean, no `error` lines:

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo check --workspace --all-targets` | Finished, 0 errors |
| `cargo clippy --workspace --all-targets -- -D warnings` | Finished, 0 errors |
| `cargo clippy --target x86_64-pc-windows-gnu -p wcore-agent -p wcore-gateway -p wcore-eval-scenarios --all-targets -- -D warnings` | Finished, 0 errors |

The Windows gate earned its place: it caught an unused import that Linux
clippy could not see, because the whole probe is `#![cfg(unix)]`.

`cargo test -p wcore-agent` (the lib, 173 integration binaries and the doc
tests) at default parallelism, five consecutive runs:

| run | failing tests | which |
|---|---|---|
| 1 | 2 | `replay_accepts_read_only_authority_files`, `read_only_authority_replay_subprocess` |
| 2 | 2 | same |
| 3 | 2 | same |
| 4 | 2 | same |
| 5 | 2 | same |

Identical every run, and nothing else fails in any of them.

## 5. Two pre-existing failures found on the way, and how I graded them

Neither is mine, and "predates my change" is a claim I had to earn, so each was
A/B'd inside THIS worktree at THIS base — not against an older binary lying
around, which is what I reached for first and which would have been the wrong
instrument (the convenient prebuilt binary was 8.5 h older than my base commit
and disagreed with it).

Method for both: `git checkout 8d69d402 -- <the two files I changed>`, `touch`,
rebuild, run; then restore, `touch`, rebuild, run.

**A. `session_journal_test::replay_accepts_read_only_authority_files` (and its
`read_only_authority_replay_subprocess` child).** The child drops to
`nobody` and calls `SessionJournal::recovered_state` against a 0400 fixture,
and gets `PermissionDenied` on the journal path.

| arm | runs | result |
|---|---|---|
| control (base sources) | 3 | 3 FAILED |
| treatment (my sources) | 3 | 3 FAILED |

Pre-existing. I did not chase it further: `recovered_state`'s read path
(`read_journal_if_present` → `lease::open_existing_nofollow`, which opens with
`write(false)`) is untouched by this lane, so the `EACCES` is coming from
somewhere else in the recovery path and finding it is a separate job. **It
needs an owner** — it is a real red on a real host, and this note is the only
place it is currently written down.

**B. `pipeline_test::one_stage_failure_drops_exactly_one_item_to_null_preserving_order`
takes ~15 minutes.** It is what made a single suite run take ~25 minutes. It is
not a deadlock — it completes — but it is three orders of magnitude off its
siblings.

| arm | runs | result |
|---|---|---|
| control (base sources) | 3 | 3 × still running past 240 s |
| treatment (my sources) | 3 | 3 × still running past 240 s |

Pre-existing, and it is NOT the same thing as the older prebuilt binary
passing this test in 11 s: that binary predates my base by 8.5 h, so something
landed in between. **That regression window is `01:37`-to-`10:11` on
2026-08-11, and it also needs an owner.**
