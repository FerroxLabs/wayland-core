# Known-negative, verbatim — BL-F26-SC3-O1-ROLLBACK

The fix is one line of intent: the whole-tree capture leg runs in
`SqliteMode::Capture`. Reverting **only** that line to `SqliteMode::Verbatim`
restores the pre-fix behaviour (raw `std::fs::copy` per file) and leaves
everything else — including all four new tests — untouched.

The mutation applied on `hetzner-dsm`:

```
570:fn copy_tree_excluding_journal(src: &Path, dst: &Path) -> Result<(), BackupError> {
571-    copy_inner(src, dst, true, SqliteMode::Verbatim)
572-}
```

## Result: 3 of the 4 new tests FAIL

```
running 16 tests
test backup::journal::tests::a_pre_scope_record_still_reads_as_a_whole_tree_operation ... ok
test backup::journal::tests::an_unpreserved_record_undoes_nothing_because_nothing_was_touched ... ok
test backup::journal::tests::intent_is_durable_before_the_target_is_touched ... ok
test backup::journal::tests::a_crash_marker_left_by_a_killed_process_is_not_part_of_the_home ... ok
test backup::journal::tests::a_scope_entry_that_could_escape_the_target_is_refused ... ok
test backup::journal::tests::a_completed_operation_leaves_no_open_record ... ok
test backup::journal::tests::recovery_never_touches_a_record_whose_owner_is_alive ... ok
test backup::journal::tests::a_record_is_scoped_per_operation_and_per_process ... ok
test backup::journal::tests::recovery_is_idempotent ... ok
test backup::journal::tests::recovery_undoes_a_dead_owners_operation_to_the_exact_pre_operation_tree ... ok
test backup::journal::tests::a_file_merely_NAMED_like_a_database_is_still_byte_identical ... ok
test backup::journal::tests::a_scope_entry_created_by_the_operation_is_removed_on_rollback ... ok
test backup::journal::tests::a_scoped_rollback_restores_its_scope_exactly_and_touches_nothing_else ... ok
test backup::journal::tests::a_sidecar_is_dropped_only_for_its_own_database ... FAILED
test backup::journal::tests::a_rolled_back_home_gets_a_database_that_opens_and_verifies ... FAILED
test backup::journal::tests::the_undo_store_holds_a_folded_database_and_none_of_its_sidecars ... FAILED

failures:

---- backup::journal::tests::a_sidecar_is_dropped_only_for_its_own_database stdout ----

thread 'backup::journal::tests::a_sidecar_is_dropped_only_for_its_own_database' (463064) panicked at crates/wcore-cli/src/backup/journal.rs:1302:9:
assertion failed: !undo.join("memory.db-wal").exists()
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- backup::journal::tests::a_rolled_back_home_gets_a_database_that_opens_and_verifies stdout ----

thread 'backup::journal::tests::a_rolled_back_home_gets_a_database_that_opens_and_verifies' (463060) panicked at crates/wcore-cli/src/backup/journal.rs:1228:9:
rollback restored a stale -wal beside the database

---- backup::journal::tests::the_undo_store_holds_a_folded_database_and_none_of_its_sidecars stdout ----

thread 'backup::journal::tests::the_undo_store_holds_a_folded_database_and_none_of_its_sidecars' (463070) panicked at crates/wcore-cli/src/backup/journal.rs:1179:9:
the undo store carried a -wal; a restored sidecar is derived state belonging to a process that no longer exists


failures:
    backup::journal::tests::a_rolled_back_home_gets_a_database_that_opens_and_verifies
    backup::journal::tests::a_sidecar_is_dropped_only_for_its_own_database
    backup::journal::tests::the_undo_store_holds_a_folded_database_and_none_of_its_sidecars

test result: FAILED. 13 passed; 3 failed; 0 ignored; 0 measured; 1862 filtered out; finished in 0.13s

error: test failed, to rerun pass `-p wcore-cli --lib`
```

(The test name reads `a_file_merely_NAMED_...` above because this run predates the
snake-case rename clippy asked for; it is `a_file_merely_named_...` at HEAD.)

## Why the FOURTH test passing here is the point, not an omission

`a_file_merely_named_like_a_database_is_still_byte_identical` passes in **both**
arms, and it must. Its job is to prove the change is a NO-OP for a file that is
merely named like a database — the 22-byte text stub
`portability-migrate-rollback-proof.sh` writes as `$TEMPLATE/memory.db`. If it
failed in either arm, the fix would be selecting on filename rather than on
header magic, and the existing interruption proofs' byte-identity assertions
would start failing for a reason unrelated to SQLite.

So the four tests split deliberately:
- three assert the NEW behaviour and fail without it;
- one asserts the PRESERVED behaviour and must hold either way.

## Restoration verified

After restoring the fixed file:

```
570:fn copy_tree_excluding_journal(src: &Path, dst: &Path) -> Result<(), BackupError> {
571-    copy_inner(src, dst, true, SqliteMode::Capture)
572-}
=== tree clean vs HEAD? ===
(empty above = restored exactly)
```

`git diff --stat HEAD -- crates/wcore-cli/src/backup/journal.rs` printed nothing,
so the mutation left no residue in the committed tree.
