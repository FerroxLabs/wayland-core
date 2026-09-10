---
issue: 1354
repo: FerroxLabs/wayland
kind: defect
title: "Backup journal gives two operations begun in one millisecond the same record"
status: open
last_verified_commit: 5c4e53bda
criteria:
  - id: c1
    text: "Two operations begun by one process in the same millisecond get distinct ids, record files and undo directories, shown by a deterministic test that pins the stamp and goes RED with the old stamp-plus-pid id."
    state: met
    evidence: "test:crates/wcore-cli/src/backup/journal.rs::two_operations_begun_in_one_millisecond_by_one_process_get_distinct_ids"
    owner: core
    note: "MET 2026-09-10 at 057bb080a. THE REPAIR: begin_scoped now names an operation through mint_op_id, which appends a process-wide zero-padded AtomicU64 sequence to the millisecond stamp and pid, so every id one process mints is distinct and the record file and undo directory derived from it are too. THE TEST PINS THE STAMP, so it does not depend on two begin calls landing in one millisecond: it mints twice at 1_757_500_000_000 with the same pid and asserts the ids differ. GREEN at 057bb080a on hetzner-dsm via tools/remote-proof.py slot default, `cargo test -p wcore-cli --lib backup::journal`: 17 passed / 0 failed, remote_exit 0, complete true (status_sha256 6bb8cde7...). RED ARM from history, branch redarm/1354-old-id commit 155cd1587 (NOT to be merged), which restores `format!(\"{stamp_millis}-{pid}\")` and nothing else: 16 passed / 1 failed, exactly this test, `assertion left != right failed: one millisecond minted one id twice`, remote_exit 101, complete true (status_sha256 18aec5b2...). NOT CLAIMED: ids are unique within one process lifetime; a different process reusing a pid inside the same millisecond after a restart is not addressed and was not measured. PRIOR FILING, kept: NOT MET, filed 2026-09-10. OBSERVED: CI run 34485707894 (a9e8f77f5), job CI (linux-containerized), step 'Shared-process lib suite' failed at crates/wcore-cli/src/backup/journal.rs:881:9, `assert_ne!(g1.op_id(), g2.op_id(), \"two operations shared one record\")`, while the same test PASSED in the nextest leg of that run (5840/18140). CAUSE, read from the code: begin_scoped minted `format!(\"{stamp}-{pid}\")` from chrono timestamp_millis, so one process beginning two operations in one millisecond wrote both intent records to one `{op_id}.{pid}.json` and shared one `undo-{op_id}` directory. That step is a required check."
  - id: c2
    text: "Ids minted by one process in one millisecond still sort in the order begun, since list_open returns records oldest first by op_id."
    state: met
    evidence: "test:crates/wcore-cli/src/backup/journal.rs::two_operations_begun_in_one_millisecond_by_one_process_get_distinct_ids"
    owner: core
    note: "MET 2026-09-10 at 057bb080a. The same pinned-stamp test asserts `first < second` as strings, which is the comparison list_open's `sort_by(|a, b| a.op_id.cmp(&b.op_id))` uses; the sequence is zero-padded to six digits so string order equals begin order up to 1,000,000 ids per process per millisecond. Green 17/17 at 057bb080a as recorded in c1. NOT CLAIMED: ordering ACROSS processes, which was stamp-then-pid before and is unchanged."
  - id: c3
    text: "Record names still carry the owning pid (`.{pid}.json`) so live-owner detection in recovery is unchanged: a_record_is_scoped_per_operation_and_per_process and recovery_never_touches_a_record_whose_owner_is_alive pass."
    state: met
    evidence: "test:crates/wcore-cli/src/backup/journal.rs::a_record_is_scoped_per_operation_and_per_process"
    owner: core
    note: "MET 2026-09-10 at 057bb080a. The record path is still `{op_id}.{pid}.json`, and both named tests passed in the 17/17 green run recorded in c1, including a_record_is_scoped_per_operation_and_per_process, whose `.{pid}.json` assertion is the one this row names; recovery_never_touches_a_record_whose_owner_is_alive passed in the same run and in the red arm. No code outside begin_scoped parses op_id (grep of crates/wcore-cli/src/backup at 5c4e53bda: list_open sorts it, recovery joins `undo-{op_id}`, mod.rs prints it). NOT CLAIMED: this run was a targeted lib filter, not the shared-process workspace suite; that is graded by the next CI run's step 'Shared-process lib suite'."
---

Created 2026-09-10 at filing time, not retroactively. Found by CI's
shared-process lib leg on the 0.13.14 integration push; searched both trackers
for 'backup journal op_id' with no match before filing.
