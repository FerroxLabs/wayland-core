# wayland#1353 — concurrent checkpoint stores and the session quota

Host `hetzner-dsm` (Linux, 96 CPU, 251 GiB), through `tools/remote-proof.py`
slot `default`. Every run below has a JSON receipt with `remote_exit` as stated
and `"complete": true`; the receipt lines are collected in `receipts.tsv` (one
per run, keyed by name) and the full logs are under the stabilization root
`evidence/`.

## The defect, reproduced

`SessionJournal::store_effect_checkpoint` scanned the `.effects` directory
against `MAX_EFFECT_CHECKPOINT_SESSION_BYTES` (536,870,912) and then wrote,
with nothing held across the two. Two production writers share one session's
directory: the durable child result store (`durable_spawner.rs`, under that
spawner's own `mutations` lock) and the tool-effect preimage store
(`orchestration/mod.rs:1215`, `spawn_blocking`, no shared lock).

Instrument: `session_journal::fault_tests::concurrent_checkpoint_stores_cannot_jointly_exceed_the_session_quota`,
brought from probe `506aed92e`. A `cfg(test)` rendezvous
(`quota_race_gate::after_quota_scan`) sits between the scan and the quota
decision, so two stores both finish scanning before either is decided. The
directory is filled to leave room for exactly ONE 64 MiB checkpoint. The gate
now counts waiters that time out, and the test asserts `passed: 2, timed_out: 0`,
so a run in which something serialized the stores cannot pass vacuously.

RED, source `2cca6b033` (the test and gate only, production code = `b37b69505`),
receipt `proof-1789058226-28245`, `remote_exit 101`:

    QUOTA_RACE rendezvous=Rendezvous { passed: 2, timed_out: 0 } total_bytes=570425344 quota=536870912 results=[Ok(()), Ok(())]
    two concurrent stores jointly exceeded the session quota: 570425344 > 536870912 bytes

Same byte count as the ledger's filing observation.

## The repair (`a8fd1ac9c`)

A per-session in-memory admission ledger, `CheckpointQuota { reserved, released }`,
held on the `SessionJournal` handle (`Arc<Mutex<_>>`, shared by clones; an
independent open of the same journal fails closed, so one ledger per checkpoint
directory in the process). NOT the writer lock.

Per store:

1. `CheckpointAdmission::before_scan` reads `released` (O(1) critical section).
2. The directory scan runs with NO lock held (unchanged; see #1301 below).
3. `admit(scanned, len)` refuses when
   `scanned + reserved + (released_now - released_before_scan) + len > quota`,
   otherwise adds `len` to `reserved` (O(1) critical section).
4. The reservation ends immediately after publication, or through `Drop` on any
   error return or unwind.

Why a concurrent store is always counted at least once: if it ended its
reservation before our step 1, it had already published, so our scan (which
starts after step 1) lists it; if it ended between steps 1 and 3 it is in the
`released` delta; if it is still reserved at step 3 it is in `reserved`; if it
is admitted after our step 3, its own admission sees our reservation.

The price, stated rather than hidden: a concurrent store can be counted TWICE
(its partly written temporary is in the scan AND it is reserved; or it
published during the scan AND is in the delta). That can only refuse a store
near the quota under real concurrency, never admit one past it. The pre-repair
scan already double-counted a publication in its hard-link window.

## c1 — GREEN, and the red arms

GREEN, source `a8fd1ac9c`, receipt `proof-1789058481-35316`, `remote_exit 0`:
`cargo test -p wcore-agent --lib --test durable_child_store_test --test f889_write_edit_reconcile_test --test workflow_limits_test`

    lib                        2749 passed; 0 failed; 3 ignored
    durable_child_store_test     12 passed; 0 failed
    f889_write_edit_reconcile     9 passed; 0 failed
    workflow_limits_test          9 passed; 0 failed

The race test now also asserts exactly one store accepted, the other refused
with the session-quota error and leaving no published file, and the accepted
checkpoint loads back at full length. The pre-existing sequential test
`effect_checkpoint_store_enforces_session_quota_before_writing` (refusal fails
closed) and `a_checkpoint_the_session_cannot_store_degrades_to_opaque_instead_of_refusing`
both pass.

RED ARMS (scratch branches, each `a8fd1ac9c` + ONE mutation, confirmed landed on
code by `git diff` before running; NONE may be merged):

    branch / commit               mutation                                   killed by (remote_exit 101, complete)
    red-m1  a0a477063   admit ignores the ledger (`unseen = 0`)            race test: 570,425,344 > 536,870,912, rendezvous {2,0}, [Ok, Ok];
                        -- the repair removed                              unit test: "a store released during the scan must still be counted"
    red-m2  8b768f8a1   `Drop` never ends a reservation                    panic test: reserved after the panic = 67,108,868 (64 MiB + the 4-byte seed store, also leaked)
    red-m3  7c79d8eeb   one process-wide ledger for every session          c2 test: [Ok, Err(session quota)] -- rendezvous passed, so the red is the SHARED QUOTA
    red-m4  edfefdb0f   `released` counted absolutely, not since the scan  unit test: `later` refused exactly the remaining room
    red-m5  3376849f5   quota scan skips `*.tmp` entries                   c3 crash-temp test: "one crash-left byte over the quota must refuse the store: Ok(())"
    red-m6  9556bb0ea   process-wide lock from before the scan to the end  c2 test: rendezvous {passed 2, timed_out 1}, results [Ok, Ok] -- the red is SERIALIZATION

Each red arm ran only the test(s) named, filtered (`2750`/`2751 filtered out`),
so a failure cannot come from a neighbouring test. The history red arm is
`2cca6b033` above (the test on the unrepaired production code).

A gap found and closed before the fix was committed: the first draft of the
unit test admitted `later` against a scan of 0, which m4 would have survived.
It now scans the published total and asks for exactly the remaining room, and
m4 is the arm that proves the strengthening was needed.

## c2 — unrelated sessions

Instrument: `checkpoint_stores_in_different_sessions_are_neither_serialized_nor_share_a_quota`.
Two journals (`a.journal`, `b.journal`; distinct `.effects` directories,
asserted), each filled to leave room for exactly one 64 MiB checkpoint. The
rendezvous is armed for BOTH directories with `expected = 2`, so each session's
store must reach the point after its scan while the other's is parked there.

- GREEN on `a8fd1ac9c`: passes; both stores accepted, both directories within
  quota, both ledgers back to `reserved == 0`.
- m6 (a process-wide lock) reds on the rendezvous: `timed_out: 1`.
- m3 (a process-wide ledger) reds on the outcome: the second session refused.

The two arms fail on DIFFERENT assertions, so the test separates "blocked by the
other session" from "charged for the other session".

NOT claimed: the rendezvous point is after the scan and before the quota
decision. A cross-session lock taken only AFTER that point and released before
the write would not be caught by the rendezvous; by construction such a lock
would also be a broken repair, and the c1 race test reds that shape (both
stores would decide on stale scans). No such lock exists in the change.

## c3 — crash-left temporaries, and reservation leaks

Crash-left temporaries. Instrument:
`crash_left_checkpoint_temporaries_still_count_against_the_session_quota`. A
1-byte `.{digest}.4242.{uuid}.tmp` for a DIFFERENT digest (so the store's own
same-digest stale cleanup cannot remove it first) sits in a directory with room
for exactly one 64 MiB checkpoint. The store is refused by the session quota,
the temporary survives, nothing is published, and no reservation remains;
removing the temporary lets the identical store succeed, landing the directory
at exactly 536,870,912 bytes. Green on `a8fd1ac9c`; m5 reds it.

Reservation leaks. A reservation ends in exactly two places: an explicit
`drop(admission)` immediately after publication, and `Drop` on every other exit
(error return or unwind). Instrument:
`panicking_checkpoint_store_releases_its_quota_reservation`: a thread admits a
64 MiB reservation against the real journal's ledger, confirms it is held, and
panics. After the join, `reserved == 0`, and a real 64 MiB store into the
remaining 96 MiB of room succeeds. Green on `a8fd1ac9c`; m2 reds it.

- Poisoning cannot wedge the quota: every critical section on the ledger is
  saturating/wrapping integer arithmetic that cannot panic, and the lock is
  recovered with `PoisonError::into_inner` rather than refused.
- A process crash loses the ledger with the process; the next process starts at
  zero and the crashed store's temporary is counted by the scan (the crash-temp
  test above).
- `panic = "abort"` is not set for release (`Cargo.toml` `[profile.release]`),
  so unwinding is the release behaviour.

NOT claimed: an error return AFTER admission but before publication is not
driven by its own test. It takes the same `Drop` path as the unwind the panic
test drives, and forcing a real mid-write I/O failure needs a fault hook this
file does not have (and on the Linux proof host the process runs as root, so a
permission-based failure is not available).

## c4 — the #1301 isolated step (NOT MET: Linux only)

What was measured: `fix1_dispatch_budget_aborts_with_partial_result` from
`crates/wcore-agent/tests/workflow_limits_test.rs`, release build, one test
thread, before (`b37b69505`) and after (`a8fd1ac9c`), on ONE executor:
`hetzner-dsm` (Ubuntu 24.04, kernel 6.8.0-101, 96 CPU), a shared build host.

Instrument (`ab1353.sh`, `analyse1353.py` in this directory):

- Each arm's binary was built through `remote-proof.py` with
  `test --release -p wcore-agent --test workflow_limits_test --no-run`
  (receipts `c4-build-A`, `c4-build-B` in `receipts.tsv`, both `remote_exit 0`,
  complete). Both arms build to the SAME deps filename
  (`workflow_limits_test-23431e060bb79b5b`), so each was copied aside
  immediately after its own build:

      arm  source     mtime       size        sha256
      A    b37b69505  1789059957  39,485,832  e67d24ee7c9a6aa71b68781479ae4a808bb1dd2f7b6f9401d901424b7eab196c
      B    a8fd1ac9c  1789060217  39,492,096  e1de028c867dbc9d2c04856dac9bc23c94446c5e11a199aa138102d95dbbff0c

  `c4-linux-meta.txt` records the same two digests at the start of the timing
  run, so the timed files are the built files.
- The binary ran directly (NOT through `cargo nextest`, which is what the CI
  isolated step and the #1301 Windows harness invoke), `--exact` the one test,
  `--test-threads=1`, a fresh `WAYLAND_HOME` per run, 12 rounds alternating
  A,B / B,A, every sample kept (`c4-linux-samples.csv`), `load1` read before each.

Result, 2026-09-10 17:10:30Z to 17:17:31Z, 24 of 24 passed:

    arm  n   median    mean      sd     min      max
    A    12  17.209 s  17.337 s  1.261  15.110   19.194
    B    12  17.814 s  17.695 s  1.379  14.988   19.752

    paired B-A within a round: median +0.026 s, mean +0.357 s, B slower in 6 of 12
    bootstrap 95% (seed 1353, 20,000): median paired diff [-1.110, +1.898] s
    median ratio B/A 1.0351, bootstrap 95% [0.9480, 1.1052]
    load1 at sample start 35.15 .. 45.24 (median 39.95); arm medians A 39.95, B 40.45

READ THIS AS: no detectable regression, at a resolution of about 10%. Per-run
spread on this shared host is ~1.3 s (7.5%), so the interval admits anything
from a 5% speed-up to a 10% slow-down. It does NOT show the repair is free at
the scale it plausibly costs (two uncontended O(1) critical sections per store),
and it says nothing about Windows, where #1301's per-dispatch cost lives.

Surface control: the timed step really runs the changed code. `surface1353.sh`
(this directory) runs each arm's timed binary once more under
`strace -f -e trace=link,linkat` and counts hard links into a session
`.effects/` directory. Syscall counts only: strace slows the run, so these are
not timing samples.

    arm  link calls  into .effects/  returned 0  test
    A    1000        1000            1000        ok (35.03 s under strace)
    B    1000        1000            1000        ok (34.90 s under strace)

One published checkpoint per dispatch (`MAX_TOTAL_DISPATCHES = 1000`), the same
count in both arms: the repair neither adds nor refuses a store in this step.
Every one of those stores is a durable child result made under the durable
spawner's `mutations` lock, one at a time. So on this step the repair only ever
takes its UNCONTENDED path, which is what #1301 c3's per-dispatch cost is made
of. Contended stores (parallel tool-effect preimages) are not exercised by this
step and are not measured here.

Why c4 stays NOT MET: the criterion is the #1301 c3 timing, and #1301 c3 is
graded on hosted `windows-latest`. This lane took no Windows measurement and
did not burn CI. What would grade it: the #1301 lane's throwaway harness
(`f0b9199ce` on `timing/w15-1301-ab`: `.github/workflows/w15-1301-ab.yml` +
`.scratch1301/harnessab1301.ps1`) checks the base out at the workflow commit and
the fix at `FIX_SHA` into `./fix`, builds each arm's release
`workflow_limits_test` into its own target dir with
`vx cargo nextest run --locked --release -p wcore-agent --test workflow_limits_test --no-run`,
then interleaves the EXACT CI isolated-step nextest invocation per arm, two
shards of six rounds, uploading every raw sample. Pointing it at base
`b37b69505` (or the #1301 fix, once this repair is rebased onto it) and
`FIX_SHA = a8fd1ac9c` (or the rebased repair) on a throwaway branch is the
measurement c4 needs. The ideal pair is `07272c36b`-rebased vs this repair
rebased onto it, since that is what will ship.

## Composition with wayland#1301 (`07272c36b`, `aa8f42912`)

`git merge-tree --write-tree --merge-base 38f82b87f a8fd1ac9c 07272c36b` applies
the #1301 pair onto this repair with NO conflict (rc 0, tree `5bb8a7c2b`). The
instrument is live: the same command with probe `506aed92e` against its parent
reports `CONFLICT (content)` in `session_journal.rs` (rc 1). The merged tree
carries both `admission.admit(session_bytes, ...)` and
`let metadata = checkpoint_entry_metadata(&entry)`, both lanes' tests, and its
diff from `a8fd1ac9c` is exactly the #1301 pair's three files.

The merged tree was committed as SCRATCH `8ab83a98a` (branch
`w15/quota1353-compose`, parent `a8fd1ac9c`, NOT to be merged) and tested:
`cargo test -p wcore-agent --lib -- checkpoint`, receipt
`proof-1789060670-73833`, TREE `5bb8a7c2b`, `remote_exit 0`, complete:
`38 passed; 0 failed`. That includes BOTH #1301 tests
(`checkpoint_quota_sizes_only_exact_digest_names_from_the_listing`,
`published_checkpoint_names_are_exact_lowercase_digests_only`) and all five of
this repair's tests, in one binary. So a rebase of this repair onto `07272c36b`
is mechanical and the two changes pass each other's tests.

NOT claimed for the composition: no clippy, no c4 timing and no full-suite run
on the composed tree; and `aa8f42912`'s own test file
(`tests/durable_child_store_test.rs`) was not run against it.

This repair does not touch `checkpoint_directory_bytes` or the entry-sizing
rule; it only moves the decision that consumes the scan's total.

## Gates

- `cargo clippy --all-targets -- -D warnings` on `a8fd1ac9c`: `remote_exit 0`.
- `cargo clippy --target x86_64-pc-windows-gnu -p wcore-agent --all-targets -- -D warnings`
  on `a8fd1ac9c`: `remote_exit 0`. It sees `cfg(windows)` code Linux clippy is
  blind to; gnu is not msvc, and no Windows TEST run was taken.
- No hetzner test was run on macOS or Windows. The new ledger code is
  platform-neutral std; the gate is `cfg(test)` on every target and the tests
  use `set_len` fillers (sparse on Linux; not necessarily on NTFS).

## Not touched

No `SOURCE_INPUTS` file (`crates/wcore-protocol/src/contract/spec.rs`) was
modified; the only production file is `crates/wcore-agent/src/session_journal.rs`.
No corpus regeneration.
