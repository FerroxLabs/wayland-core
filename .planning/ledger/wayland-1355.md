---
issue: 1355
repo: FerroxLabs/wayland
kind: defect
title: "Session index writes give up after a 1 s lock wait under contention, and a writer can hit ENOENT"
status: open
last_verified_commit: e1c3bf704
criteria:
  - id: c1
    text: "Both errors are root-caused with the instrument that named them -- the 1 s give-up under contention AND the ENOENT -- each with a deterministic red test that does not depend on CI load (a held lock, an injected stall)."
    state: met
    evidence: "test:crates/wcore-agent/src/session.rs::test_1355_a_parked_writer_reports_enoent_when_its_store_is_removed"
    owner: core
    note: "MET 2026-09-10, hetzner-dsm (Linux 6.8.0-101, 96 CPU) via tools/remote-proof.py slot default, cargo test (one shared process). OBSERVED, as filed: run 34476606238 (f4563ffe6), ci-diagnostics shared-lib/stdout.log, session.rs:1624:55 panics with `Could not acquire index lock after 1s` and `No such file or directory (os error 2)`. CAUSE 1, the give-up, named by CI's own panic text and the code at base 5b7c18412 session.rs:1016: acquire_sentinel_lock applied a FIXED 1 s spin budget to writers in the SAME process, whose hold includes atomic_write's sync_all of index.json. Deterministic red test test_1355_writer_waits_out_an_in_process_hold_past_one_second releases the holder only after a cfg(test) recorder has SEEN the waiter contend 1.5 s in, which the old loop cannot survive: RED at 9cd1e5a2d (tests only), remote_exit 101, 5 passed 1 failed on exactly that message. CAUSE 2, the ENOENT, is a TEARDOWN ARTIFACT of the test, named by the ORDER of libtest's shared capture buffer (stdout.log 2945/2948 the two give-ups, 2951 the test thread's own panic at `h.join().unwrap()` session.rs:1630, 2954 the ENOENT): the first failed writer unwound the test thread, dropping its TempDir under writers still running, and a writer parked in acquire_sentinel_lock hit create_new in the removed directory, returned raw. By exhaustion every ENOENT-producing step of persist_first_message needs a missing parent, and a lock-lifecycle race yields a second holder, never ENOENT. The evidence test reproduces it deterministically (NotFound, raw os error 2 on unix) and is green on every arm: it is the mechanism, not the repair. NOT CLAIMED: why CI's holds were that long -- on this host the OLD binary's worst loaded in-process wait was 516,941 us, so CI's latency was not reproduced. Detail: .planning/evidence/w15-1355/README.md."
  - id: c2
    text: "A legitimate index write under contention no longer fails because another in-process writer holds the lock longer than a fixed 1 s; stale-lock recovery after a killed writer still works (test); mutual exclusion is not weakened -- no two holders at once and no lost index entry -- shown with n>=100 iterations of the 10-thread test under load at --retries 0 with 0 failures."
    state: met
    evidence: "test:crates/wcore-agent/src/session.rs::test_1355_writer_waits_out_an_in_process_hold_past_one_second"
    owner: core
    note: "MET 2026-09-10, hetzner-dsm. REPAIR e1c3bf704 (session.rs only): writers of one process queue on a per-canonical-directory slot, InProcessIndexLock, taken BEFORE the sentinel and released only AFTER it is removed, bounded by the same 30 s the sentinel treats as a dead holder; a waiter at the bound errors and never steals. Only the queue head touches the sentinel. DISCRIMINATOR, the evidence test: RED at 9cd1e5a2d, RED on RED ARM branch w15/idxlock1355-red-c2 at 516a6b3c2 (the fix tree with only the queue removed; remote_exit 101, complete true, same message), green at e1c3bf704. At e1c3bf704: full `cargo test -p wcore-agent --lib` 2744 passed 0 failed in one process; `clippy -p wcore-agent --all-targets -- -D warnings` remote_exit 0. KILLED WRITER: test_1355_a_killed_writers_index_lock_is_recovered kills a real child process inside the lock, asserts its pid is in the sentinel left behind, ages only the mtime past 30 s, and recovers. NO STEAL: test_1355_a_live_foreign_holders_sentinel_is_never_stolen. EXCLUSION: test_f033_index_lock_parallel now asserts a holder-gauge maximum of exactly 1 and listed ids == committed ids. LOAD, fix binary sha256 4ffa97a0 (content-checked: it carries the new error string and the old binary 332474bb does not), each iteration its own libtest process, no retries: 200/200 of `--exact test_f033_index_lock_parallel` pinned to 1 CPU with 32 busy loops and 2 O_DSYNC writers, every run 10 acquisitions and 10 writes, worst wait 663,502 us; and 100/100 of the 40 `session::` tests in one process at --test-threads 16 on 2 pinned CPUs with 12 busy loops, worst wait 97,515 us. NOT CLAIMED: the load arms do NOT discriminate -- the OLD binary also passed 50/50, 50/50, 10/10 and 5/5 (test-level) under the same loads and never waited 1 s here, so they prove exclusion under load, and the deterministic test proves the give-up is gone. Writers in DIFFERENT processes keep the fixed 1 s budget, and the cross-process stale-steal and release-by-path races after a >30 s hold are unchanged."
  - id: c3
    text: "The ENOENT is repaired or proven unreachable in production, with c1's red test green."
    state: met
    evidence: "test:crates/wcore-agent/src/session.rs::test_1355_a_failed_writer_does_not_remove_the_store_under_a_parked_sibling"
    owner: core
    note: "MET 2026-09-10 as REPAIRED, for the ENOENT CI observed, whose cause was the test harness (c1). Repair: join_every_writer joins every writer before any is judged, so the store outlives all of them and each failure reports its own cause. The evidence test injects a failing writer while a sibling is parked on the lock: RED on RED ARM branch w15/idxlock1355-red-harness at 439d54417 (the fix tree with the old unwinding join restored; remote_exit 101, complete true, panicked at session.rs:1422 on the injected failure), green at e1c3bf704. NOT CLAIMED, AND A CORRECTION: the ENOENT CLASS is NOT unreachable in production. `backup restore --replace` clears the target home, sessions/ included, with remove_dir_all (crates/wcore-cli/src/backup/restore.rs:234 clear_target; journal rollback at backup/journal.rs:722) and checks only for a live RESTORE owner, not a live engine on that home, so an engine in another process at the index lock then gets the same bare os error 2. Failing is correct there, but the message does not name the path; unchanged by this lane."
  - id: c4
    text: "The .config/flaky-allowlist.txt row for test_f033_index_lock_parallel is DELETED, not renewed, when c2 and c3 land."
    state: met
    evidence: "absent:.config/flaky-allowlist.txt::test_f033_index_lock_parallel"
    owner: core
    note: "MET 2026-09-10. Deleted after c2 and c3 were met on e1c3bf704: the row at line 63 (expiry 2026-09-15, gh#1169). It recorded the test as never measured at --retries 0, and the shared-process lib step honours no allowlist, so it never protected the required check it reddened. The absent: token re-reads the file every gate run, so a merge that resurrects the row reddens this criterion."
---

Created 2026-09-10 at filing time, not retroactively. Found while diagnosing
the step-37 red on the 0.13.14 integration push; the same required step went
red on 4 of the 9 most recent runs where it ran, three distinct flakes.

Graded 2026-09-10 by lane w15/idxlock1355. Two causes, two tests. The give-up
was a fixed 1 s spin budget applied to writers inside one process. The ENOENT
was the old test unwinding through its own TempDir while writers still ran.
Numbers, receipts and red arms are in `.planning/evidence/w15-1355/README.md`.
