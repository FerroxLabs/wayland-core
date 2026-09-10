---
issue: 1357
repo: FerroxLabs/wayland
kind: defect
title: "Concurrent effect-checkpoint stores fail spuriously on each other's temporary files"
status: open
last_verified_commit: b348328cc
criteria:
  - id: c1
    text: "Both failures are reproduced deterministically by tests that force the interleaving, red on the current code: two same-digest stores, and a store whose scan races another store's temp removal."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10 from adversarial review of the wayland#1353 quota repair (read-only, at wayland-core a8fd1ac9c; findings F2 and F3). PRE-EXISTING, not introduced by #1353. READ FROM THE CODE, not reproduced (line numbers crates/wcore-agent/src/session_journal.rs at a8fd1ac9c): (1) store_effect_checkpoint calls remove_stale_checkpoint_temps (:766), which deletes every `.{digest}.*.tmp` for the digest (:1458, :1472-1481) whether or not another LIVE store is writing it; that store's hard_link (:802) then fails NotFound -> I/O error (:812); conversely A unlinking its own temp (:808) between B's listing and B's stat/remove (:1462, :1473/:1478) fails B NotFound; load_effect_checkpoint has the same blanket removal when nlink > 1 (:857-862). (2) checkpoint_directory_bytes treats NotFound from stat as a hard error (:1498-1501), so any concurrent store removing its temp between the listing and the stat fails the scanning store; wayland-core 07272c36b (#1301) still sizes temps with symlink_metadata, so the composed tree keeps it. CONSEQUENCE AS READ: on the tool-effect preimage path the failed store degrades that tool call's crash recovery to opaque (orchestration/mod.rs:2189-2201) -- no refusal, no data loss; durable-child consequence not traced. REACH AS READ: parallel tool batches (join_all, orchestration/mod.rs:749), including edits of byte-identical files, and concurrent stores of different digests for (2). Trackers searched for 'remove_stale_checkpoint_temps', 'checkpoint temp', 'checkpoint_directory_bytes', 'degrades to opaque' and 'effect checkpoint concurrent' in both repos: no matching ticket (control 'session quota' returned #1353 in the same session)."
  - id: c2
    text: "A store never deletes a temporary file another LIVE store is writing; crash-left temporaries are still cleaned up and still counted against the quota (wayland#1353 c3's test still passes); a vanished entry during a scan is treated as gone, not as an error, without weakening the quota (wayland#1353's race test still passes)."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. Must compose with wayland#1353's admission ledger and wayland#1301's published-name sizing (07272c36b)."
  - id: c3
    text: "The c1 tests are green, with a red arm from history for each repair."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10."
---

Created 2026-09-10 at filing time, not retroactively. Found by adversarial
review of the wayland#1353 repair; both paths predate it.
