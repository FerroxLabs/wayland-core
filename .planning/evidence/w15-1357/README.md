# wayland#1353 review follow-up and wayland#1357

Host `hetzner-dsm` (Linux, 96 CPU) through `tools/remote-proof.py` slot
`default`. Every run below has a receipt with `remote_exit` as stated and
`"complete": true` (lines collected in `receipts.tsv`). Base: integration
`b43d27551`.

AN INSTRUMENT GAP FOUND AND CLOSED: `-- checkpoint` is a libtest substring
filter. `a_store_published_after_another_stores_scan_is_still_counted`,
`a_temporary_removed_during_stale_temporary_cleanup_is_already_gone` and
`an_entry_removed_during_the_quota_scan_counts_as_gone` do not contain that word,
so a `-- checkpoint` run silently never selects them. Its 37-pass count on
`d0d69d361` does NOT include the first. Every green below names its tests.

## Part A — #1353: pin the two orderings the quota proof relies on

Commit `d0d69d361` (test instrumentation only; no production behaviour change).

The review's premise, checked and partly disagreed with. A single test that
parks Y after its scan while X runs to completion catches (a), snapshot taken
after the scan. It CANNOT catch (b), reservation ended before publication: in
the repair Y's snapshot precedes its scan, so it precedes ALL of X, and X's
release lands in Y's delta wherever X releases. Under (b) with the drop after
the write and before `hard_link`, X's fully written temporary is visible to any
scan between release and link, so the unsafe window is a directory-listing-order
race (published name created behind the iterator, temporary removed ahead of
it), which no test can force without instrumenting the sizing function. So (b)
is pinned at the ordering itself.

- `a_store_published_after_another_stores_scan_is_still_counted`: Y parked
  immediately after its listing (the scan now runs through
  `scan_checkpoint_quota`, so nothing can be placed between listing and park);
  X stores, publishes and releases completely; Y decides.
- `a_checkpoint_reservation_ends_only_after_its_checkpoint_is_published`: a
  thread-local hook at the instant a reservation ends records whether the
  published checkpoint exists.

GREEN, `d0d69d361`, the three tests named (`pa-green-named`), `remote_exit 0`:
`3 passed; 0 failed`.

RED ARMS (scratch branches, each `d0d69d361` + ONE line, confirmed by
`git diff`; NONE may be merged). Each run named the same three tests:

    arm                                   mutation                                   result (remote_exit 101)
    w15/quota1353-review-red-a  4961de50f  snapshot taken after the scan              park test FAILED: "Y was admitted on a scan that could not
                                                                                      see X: 570425344 > 536870912 bytes (Ok(()))"; race test ok,
                                                                                      observer ok
    w15/quota1353-review-red-b  75f52c633  drop(admission) after sync, before         observer FAILED: left Some(false) "the reservation ended
                                           hard_link (the review's literal (b))       before the checkpoint was published"; park test ok, race ok
    w15/quota1353-review-red-b2 2fd215c7b  drop(admission) right after admit,         observer FAILED: left Some(false); park test ok, race ok
                                           before the temporary exists

So each ordering is pinned by exactly one test, the pre-existing race test is
green under all three (the review's gap, confirmed), and the park-after-scan
test is green under both (b) arms (the premise disagreement, confirmed by
measurement rather than argument).

History note: the first cut of this commit (`a5124fd43`) did not compile
(`E0063`, a `Gate` literal missing the new fields; `proof-...` exit 101 at
build). It was amended before any result was taken from it, rebased onto
`b43d27551` as `d0d69d361`, and every red arm above was recut from `d0d69d361`.

## Part B — #1357

### c1: the defects, reproduced deterministically (`2302378f5`, tests only)

New `cfg(test)` park points (test instrumentation, no production behaviour):
`BeforeLink` / `AfterLink` in the store, `CleanupListed` / `CleanupStatted` in
stale-temporary cleanup, `ScanListed` at the top of the quota scan's loop. A
park-first gate holds ONLY the first store to reach the armed point.

- T1 `a_store_never_deletes_the_live_temporary_of_another_store_of_the_same_checkpoint`:
  the first store has written and synced its temporary and is parked before its
  link; a second store of the same checkpoint runs to completion.
- T2 (unix) `loading_a_checkpoint_never_deletes_the_live_temporary_of_a_store_not_yet_linked`:
  a pending store is parked before its link; the same checkpoint is published
  beside it with a crash-left hard-link alias; `load_effect_checkpoint` runs.
- T3 `a_temporary_removed_during_stale_temporary_cleanup_is_already_gone`: a
  crash-left temporary is removed while cleanup is parked at `CleanupListed`,
  then (second stage) at `CleanupStatted`.
- T4 `an_entry_removed_during_the_quota_scan_counts_as_gone`: another temporary is
  removed while the scan is parked on its listing; room is left so the store
  fits ONLY if that 4,096-byte entry counts as gone.
- T5 (unix) `loading_a_checkpoint_while_its_store_still_links_its_temporary_succeeds`:
  a reader loads while its store is parked after its link. NOT a defect test: a
  guard, green before and after, that the repair still removes a live
  temporary when it is only a redundant link.

RED on the unrepaired store, `2302378f5`, the five named (`pb-red`),
`remote_exit 101`, `1 passed; 4 failed`:

    T1  FAILED  first store Err(Io NotFound) at its link: "the second store deleted the first store's live temporary"
    T2  FAILED  pending store Err(Io NotFound): "loading deleted a live store's temporary"
    T3  FAILED  CleanupListed stage: Err(Io NotFound) on the crash-left temporary
    T4  FAILED  Err(Io NotFound) on the other temporary: "an entry removed during the scan failed the store"
    T5  ok

NOT CLAIMED for c1: T3 panicked in its FIRST stage, so its `CleanupStatted`
stage (vanished between stat and removal) did not run on the unrepaired code;
that site is instead shown by the `remove` red arm against the repair.

### c2/c3: the repair (`4c8bb5219`)

- A per-journal set of LIVE temporary names (`checkpoint_temporaries`, shared by
  clones like the quota ledger). A store registers its temporary's name BEFORE
  creating the file and unregisters it on every exit (a guard's `Drop`).
- Why no pid or age rule is needed: the writer lease makes one handle family the
  only writer to a session's checkpoint directory, so an unregistered temporary
  belongs to no live store. VERIFIED, not assumed: the lease is exclusive in
  process and across processes on Unix (`try_lock`) and Windows (`LockFileEx` on a
  sentinel range); the ungated `compacted_replacement_keeps_the_data_inode_lock`
  (second in-process opener refused) and
  `operating_system_releases_writer_lease_after_process_exit` (second process
  refused) PASS on the Windows CI leg: run 34351599352 (main 3d3856d40, job
  `CI (Array)`, runs-on windows-latest, success), whose JUnit artifact also
  carries Windows-only suites (`wcore-tools::windows_nul_device_probe`).
  `lease.rs` is byte-identical between 3d3856d40 and b43d27551. The planning
  records that call the lease `#[cfg(unix)]`-gated (T-22-06, F-12) predate the
  Windows implementation; only the goal-fleet second-opener test is still gated.
- Cleanup skips a registered temporary UNLESS it is already a second hard link to
  the published checkpoint (Unix `dev`/`ino`; that removal only drops a redundant
  name its store ignores). A temporary gone before its stat or removal is gone.
- `scan_checkpoint_quota` lists again when an entry vanishes between listing and
  stat, at most 8 scans, failing closed past that. Every rescan is still after the
  admission's released snapshot, so the quota is not weakened.
  `checkpoint_directory_bytes` and its sizing are untouched (one `cfg(test)` hook).
- A guard added in `8e9cf00dc`: `a_stale_temporary_of_the_same_checkpoint_is_still_removed`.
  No earlier test planted an UNLINKED, unregistered temporary of the same digest,
  so "crash-left temporaries are still cleaned" was not pinned.

GREEN on the repair `4c8bb5219`:

- `pb-green-named`, all 12 quota and temporary tests NAMED (T1-T5 plus the seven
  #1353 tests), `remote_exit 0`, `12 passed; 0 failed`.
- `pb-green-full`, `test -p wcore-agent --lib --test durable_child_store_test
  --test f889_write_edit_reconcile_test --test workflow_limits_test --test
  session_journal_test`, `remote_exit 0`: lib 2757 passed / 0 failed / 3 ignored,
  durable_child_store_test 12, f889 9, session_journal_test 52, workflow_limits 9.

RED ARMS, one per repair (scratch, never merge; each `4c8bb5219` + ONE line,
confirmed by `git diff`; each run named T1-T5; all `remote_exit 101`, complete):

    arm (branch w15/quota1357-red-*)  mutation                                     result
    registry  e4c4a1312   nothing is ever registered live                    T1, T2 FAILED (the history messages); T3, T4, T5 ok
    link      00a86f25b   live links to the checkpoint are kept              T5 FAILED only: "checkpoint has unsafe links or permissions"
    stat      ab62ce134   NotFound before cleanup's stat is an error again   T3 FAILED only, at CleanupListed
    remove    735ed9618   NotFound before cleanup's removal is an error      T3 FAILED only, at CleanupStatted (its first stage passed)
    rescan    6d3cb5909   no rescan for an entry vanished during the scan    T4 FAILED only: "an entry removed during the scan failed the store"

Each repair is pinned by its own test and no arm is caught by another's. The
`remove` arm also closes c1's note: the CleanupStatted site that did not run on
the unrepaired code is shown red here.

FINAL CODE TIP `8e9cf00dc` (the repair plus the T6 guard):

- `pd-green-named`, 15 tests NAMED (the seven #1353 quota tests, T1-T5, T6,
  `effect_checkpoint_repairs_crash_after_publication_link`,
  `effect_checkpoint_store_enforces_session_quota_before_writing`),
  `remote_exit 0`, `15 passed; 0 failed`.
- `pd-red-dead`, branch `w15/quota1357-red-dead` `6ea729fad` (`8e9cf00dc` + one
  line: cleanup treats every temporary as live), `remote_exit 101`: T6 FAILED only,
  "published=false: the crash-left temporary was not removed", while
  `effect_checkpoint_repairs_crash_after_publication_link` stayed GREEN, which is
  why that older test could not pin crash-left cleanup (its alias is a hard link).
- `pd-history-t6`, branch `w15/quota1357-history-t6` `185c7335c` (`2302378f5`, the
  unrepaired store, plus T6 only), `remote_exit 0`, `1 passed`: T6 describes
  behaviour the store already had, which the repair keeps.

## Composition with #1301 (`07272c36b`)

Integration `b43d27551` does NOT contain `07272c36b` (`checkpoint_entry_metadata`
is absent), so composition was checked explicitly.
`git merge-tree --write-tree --merge-base 38f82b87f 4c8bb5219 07272c36b` applies
the #1301 pair (`aa8f42912`, `07272c36b`) onto the #1357 repair with NO conflict
(rc 0, tree `1011de6ad`). Control, same command with probe `506aed92e` against its
parent: `CONFLICT (content)` in `session_journal.rs`, so the instrument can fail.
The merged tree was committed as SCRATCH `b4c2dfb48` (branch
`w15/quota1357-compose`, never merge); its diff from `4c8bb5219` is exactly the
#1301 pair's three files, and it carries both `checkpoint_entry_metadata` and this
lane's `scan_checkpoint_quota`, `LiveCheckpointTemporary` and `ScanListed` hook.

`pb-compose`, `b4c2dfb48`, 14 tests NAMED (the 12 quota and temporary tests plus
#1301's `checkpoint_quota_sizes_only_exact_digest_names_from_the_listing` and
`published_checkpoint_names_are_exact_lowercase_digests_only`), `remote_exit 0`,
`14 passed; 0 failed`.

The only line this lane added inside `checkpoint_directory_bytes` is the
`cfg(test)` `ScanListed` hook at the top of its loop, five lines above the line
`07272c36b` changes; the vanished-entry repair lives in `scan_checkpoint_quota`, so
the sizing code #1301 changes is untouched. NOT CLAIMED for the composed tree: no
clippy, no full suite, and T6 (added after) was not in this run.

## Gates

- `cargo clippy --all-targets -- -D warnings`: `remote_exit 0` on `4c8bb5219`
  (`pb-clippy`) and on `8e9cf00dc` (`pd-clippy`).
- `cargo clippy --target x86_64-pc-windows-gnu -p wcore-agent --all-targets -- -D warnings`:
  `remote_exit 0` on `4c8bb5219` (`pb-clippy-win`) and `8e9cf00dc` (`pd-clippy-win`).
  gnu is not msvc, and no Windows TEST run was taken.
- `python3 scripts/check-no-personal-identifiers.py` and
  `python3 scripts/check-criteria-ledger.py --offline`, run inside this worktree,
  before the evidence commit (results in the commit message).
- Redaction: `.planning/evidence/w15-1353/mutate1353.py` (committed earlier by this
  lane, already in integration) carried an absolute home path naming the host user;
  it now uses `pathlib.Path.home()`. The identifier checker reports home paths but
  does not fail on them, so it did not catch this. The two mutation scripts in this
  directory use the same form; run scripts are not included (they embed session
  scratch paths).

## NOT touched, NOT claimed

- No `SOURCE_INPUTS` file (`crates/wcore-protocol/src/contract/spec.rs`) changed;
  the only production file is `crates/wcore-agent/src/session_journal.rs`.
- No macOS or Windows TEST run; Linux (hetzner) only, plus both clippy targets.
- #1301 c4 per-dispatch cost is not re-measured. The repair adds, per store, one
  uncontended lock with a set insert and remove, and per same-digest temporary in
  cleanup one set lookup; the quota scan gains nothing outside a vanished-entry race.
- Not measured: how often production concurrency reaches these windows; the park
  points FORCE them.
- Durable-child consequence of a failed store (the ticket noted it untraced) is not
  traced here either; the repair removes the failures themselves.
