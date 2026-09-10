---
issue: 1355
repo: FerroxLabs/wayland
kind: defect
title: "Session index writes give up after a 1 s lock wait under contention, and a writer can hit ENOENT"
status: open
last_verified_commit: 057bb080a
criteria:
  - id: c1
    text: "Both errors are root-caused with the instrument that named them -- the 1 s give-up under contention AND the ENOENT -- each with a deterministic red test that does not depend on CI load (a held lock, an injected stall)."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. OBSERVED in CI's shared-process lib step (cargo test --workspace --lib, required check, no retries): session::tests::test_f033_index_lock_parallel failed on run 34476606238 (f4563ffe6) and run 34224918128 (d93fd7960, 0.13.13 integration). The ci-diagnostics-linux-f4563ffe6... artifact, shared-lib/stdout.log, shows threads panicking at crates/wcore-agent/src/session.rs:1624:55 (`manager.persist_first_message(&s).unwrap()`) with `Could not acquire index lock after 1s` AND, in another thread of the same run, `No such file or directory (os error 2)`. READ FROM THE CODE: acquire_sentinel_lock (session.rs, called by with_index_lock) spin-waits 10 ms steps to a 1 s deadline, then bails; it steals only a lock older than 30 s. The ENOENT is NOT explained by that budget, and atomic_write stages a unique tempfile temp, so a shared temp name is not its cause. PRODUCTION REACH: update_index_for from engine.rs:22165 (error logged, index update dropped) and session.rs:554/:739 (propagated); list and cleanup_old take the same lock."
  - id: c2
    text: "A legitimate index write under contention no longer fails because another in-process writer holds the lock longer than a fixed 1 s; stale-lock recovery after a killed writer still works (test); mutual exclusion is not weakened -- no two holders at once and no lost index entry -- shown with n>=100 iterations of the 10-thread test under load at --retries 0 with 0 failures."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10."
  - id: c3
    text: "The ENOENT is repaired or proven unreachable in production, with c1's red test green."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10."
  - id: c4
    text: "The .config/flaky-allowlist.txt row for test_f033_index_lock_parallel is DELETED, not renewed, when c2 and c3 land."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, filed 2026-09-10. The row (line 63, expiry 2026-09-15, closed gh#1169) records the test was NEVER measured at --retries 0; the shared-process step honours no allowlist, so the row never protected the required check it reddened. Trackers searched for 'test_f033_index_lock_parallel' and 'f033 index lock' in both repos: no matching ticket (control 'quota' returned #1353 in the same session)."
---

Created 2026-09-10 at filing time, not retroactively. Found while diagnosing
the step-37 red on the 0.13.14 integration push; the same required step went
red on 4 of the 9 most recent runs where it ran, three distinct flakes.
