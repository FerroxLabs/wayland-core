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
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. OBSERVED: CI run 34485707894 (a9e8f77f5), job CI (linux-containerized), step 'Shared-process lib suite' failed at crates/wcore-cli/src/backup/journal.rs:881:9, `assert_ne!(g1.op_id(), g2.op_id(), \"two operations shared one record\")`, while the same test PASSED in the nextest leg of that run (5840/18140). CAUSE, read from the code: begin_scoped minted `format!(\"{stamp}-{pid}\")` from chrono timestamp_millis, so one process beginning two operations in one millisecond wrote both intent records to one `{op_id}.{pid}.json` and shared one `undo-{op_id}` directory. That step is a required check."
  - id: c2
    text: "Ids minted by one process in one millisecond still sort in the order begun, since list_open returns records oldest first by op_id."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10."
  - id: c3
    text: "Record names still carry the owning pid (`.{pid}.json`) so live-owner detection in recovery is unchanged: a_record_is_scoped_per_operation_and_per_process and recovery_never_touches_a_record_whose_owner_is_alive pass."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10."
---

Created 2026-09-10 at filing time, not retroactively. Found by CI's
shared-process lib leg on the 0.13.14 integration push; searched both trackers
for 'backup journal op_id' with no match before filing.
