---
issue: 1303
repo: FerroxLabs/wayland
kind: defect
title: "Windows: a racing chunked credential write fails outright with ACCESS_DENIED (os error 5), and the caller loses a single-use refresh token"
status: open
last_verified_commit: bdd687b29
criteria:
  - id: c1
    text: "The Windows file operation that answers ERROR_ACCESS_DENIED is identified BY FRAME rather than inferred, and the answer distinguishes lock acquisition from manifest publish."
    state: met
    evidence: "symbol:crates/wcore-config/src/credentials.rs::acquire_with"
    owner: core
    note: "MET at c1d4d77b8. THE FRAME IS THE LOCK ACQUISITION, not the manifest publish, and it is settled by ENUMERATION rather than by the delete-pending story. In the failing test the store is `Shared { entries: Mutex<HashMap<String, String>> }` -- an in-memory map -- so no manifest publish touches a filesystem at all and cannot produce an io::Error under any schedule. Every OTHER filesystem call reachable from chunked_put there is discarded and cannot propagate: is_stale's metadata (`.unwrap_or(false)`), the stale-steal remove_file, and Drop's read_to_string/remove_file (all `let _ =`). secure_credential_dir is create_dir_all plus a `#[cfg(unix)]` chmod, and create_dir_all on an existing directory returns Ok. That leaves exactly ONE propagated call: the `create_new(true).write(true).open(path)` inside the acquisition loop, whose PermissionDenied answer fell through to `Err(e) => return Err(CredentialsError::Io(e))`. MEASURED ON REAL WINDOWS, not inferred: on Windows 11 build 26200, a lockfile put into delete-pending (FileDispositionInfo with the handle still open) answers CREATE_NEW with `kind=PermissionDenied raw=Some(5) msg=Access is denied. (os error 5)` -- byte-identical to the CI payload -- and `is it the AlreadyExists the lock special-cased? false`, which is why the one contention arm missed it. Once the last handle closes the same CREATE_NEW returns Ok, so the state is transient. WHAT WOULD FALSIFY THIS: a payload from the same test whose io::Error can be traced to a store that touches the filesystem, or a Windows measurement in which a delete-pending name answers CREATE_NEW with ERROR_FILE_EXISTS. RE-VERIFIED AT 968996faf, 56 commits past the tree this row was graded on, rather than carried forward: the enumeration argument still holds against the source in this tree, and the behaviour was re-measured on SeanDesktop 2026-09-10 -- see c2."
  - id: c2
    text: "A concurrent chunked_put on Windows never returns Err for a race it is supposed to serialise: a regression test exercises the LOSING writer and asserts it either commits whole or retries, measured on Windows at --retries 0."
    state: met
    evidence: "test:crates/wcore-config/src/credentials.rs::a_losing_writer_denied_by_a_delete_pending_lockfile_still_commits_whole"
    owner: core
    note: "MET at 2a67a204f. The measurement the previous note said was missing was taken on 2026-09-10 on SEANDESKTOP -- the ferrox-win-msvc host -- against a checkout pinned to the exact candidate commit 2a67a204f4cd87a97bd604a2bec0e16c98396595, verified by `git rev-parse HEAD` before each run and with a clean worktree. TWO ARMS, because an isolated pass is precisely the evidence this ticket's own allowlist entry says does not count. ISOLATED: `cargo nextest run -p wcore-config --lib --retries 0 -E test(chunk_write_lock_verification)`, n=20 consecutive runs, 20 PASS / 0 FAIL, 6 tests each, wall 3.823-4.100s (spread 1.07x). CONTENDED, which is the arm that matters: the full 809-test wcore-config lib suite at --retries 0, n=5, where all three named tests passed in every run alongside the pathological chunk_crash_injection load -- a_write_that_cannot_take_the_lock_refuses_rather_than_racing 0.234s, a_losing_writer_denied_by_a_delete_pending_lockfile_still_commits_whole 0.276s, a_second_writer_cannot_commit_over_a_parked_writers_parts 0.435s, racing_writers_never_yield_a_spliced_credential 3.964s. Zero PermissionDenied, zero spliced credentials, across 25 total gradings of the losing-writer test. RE-MEASURED AT 968996faf on 2026-09-10, on the same host, because a row graded 56 commits back is a row nobody has run: ISOLATED `-E test(credentials::chunk_write_lock_verification::)` at --retries 0, n=20 consecutive, 20 PASS / 0 FAIL, wall 3.991-4.083s (spread 1.02x). CONTENDED, the full 817-test wcore-config lib suite at --profile ci --retries 0 --no-fail-fast, n=5, all five runs 817/817 passed with the pathological chunk_crash_injection load alongside -- a_dead_holders_lock_is_stolen 0.151-0.169s, a_write_that_cannot_take_the_lock_refuses_rather_than_racing 0.222-0.241s, a_losing_writer_denied_by_a_delete_pending_lockfile_still_commits_whole 0.257-0.386s, a_second_writer_cannot_commit_over_a_parked_writers_parts 0.414-0.442s. Zero PermissionDenied anywhere. That is a further 25 gradings of the losing-writer test, 50 in total across the two commits. STILL NOT THE HOSTED-RUNNER RATE, and c4 continues to record that as unmeasured. WHAT THIS IS NOT: it is not the HOSTED-RUNNER rate, and c4 continues to record that as unmeasured. Two environment faults were caught and discarded before these numbers rather than reported as results -- a machine-level CARGO_TARGET_DIR pointing at a non-existent F: drive killed 20 iterations in ~1s each having run nothing, and TMP/TEMP pointing at F:\\Temp\\Codex failed the LINK step; both runs were rejected by an explicit build-exit guard as NOT A TEST RESULT."
  - id: c3
    text: "Both chunk_write_lock_verification tests come off .config/flaky-allowlist.txt, and the entries are DELETED rather than renewed."
    state: met
    evidence: "absent:.config/flaky-allowlist.txt::racing_writers_never_yield_a_spliced_credential"
    owner: core
    note: "PROVENANCE AFTER A REBASE, stated so the SHAs in this note can be checked: every Windows, Linux and macOS figure here was taken at commit a321f13982513dfe96d751c0fa9276905d071760, which was rebased onto the macOS-compile fix and now ships as bdd687b29. The measurement is unaffected and that is verified rather than asserted -- `crates/wcore-config/src/credentials.rs` is blob 6d205261fb772d37f2de3ea6365939a09270d093 at BOTH commits, against a different blob at the pre-fix base, so the tree under test is byte-identical. a321f13982 itself stays reachable at `refs/heads/lane/w15-wincred`. MET. Both `chunk_write_lock_verification` entries are DELETED from `.config/flaky-allowlist.txt`, not renewed: lines 86 and 87 (`racing_writers_never_yield_a_spliced_credential` and `a_second_writer_cannot_commit_over_a_parked_writers_parts`, both gh#1303) are gone, and the file's remaining entry count is 23. THE ORDER THIS ROW SET FOR ITSELF WAS FOLLOWED: land the fix, satisfy c2, then delete both lines in the same change. c2 is met and re-measured at two commits (50 gradings of the losing-writer test). WHAT THIS CHANGE ADDS ON TOP OF c2, on SeanDesktop (the `ferrox-win-msvc` host) at a321f1398 with the tree asserted clean: 5 consecutive runs of the FULL 817-test wcore-config lib suite at `--profile ci --retries 0 --no-fail-fast` -- the contended arm, not an isolated filter, because an isolated pass is precisely the evidence this ticket's own allowlist entry said does not count. All six `credentials::chunk_write_lock_verification::` tests passed in all 5 runs (`lock_identity_follows_the_service_and_key` 0.101-0.147s, `a_dead_holders_lock_is_recovered_rather_than_wedging_writes` 0.155-0.199s, `a_write_that_cannot_take_the_lock_refuses_rather_than_racing` 0.222-0.271s, `a_losing_writer_denied_by_a_delete_pending_lockfile_still_commits_whole` 0.268-0.311s, `a_second_writer_cannot_commit_over_a_parked_writers_parts` 0.426-0.465s, `racing_writers_never_yield_a_spliced_credential` 4.623-4.674s). Zero PermissionDenied in any run. THE CONTENTION CONDITION CHANGED UNDERNEATH THIS ROW AND IT IS SAID PLAINLY RATHER THAN CLAIMED AS EXTRA CREDIT: the pathological `chunk_crash_injection` load these tests used to run alongside is 40x cheaper after the wayland#1300 fix (817-test suite 14.60-15.00s against 145-146s), so these 5 runs apply LESS concurrent pressure than the runs recorded under c2, not more. They are additional gradings at a real `--retries 0`, and they are not a harder arm. WHAT IS STILL NOT MEASURED, and is why c4 stays as it is rather than being quietly upgraded: the rate on a hosted Windows runner. Deleting these two rows asserts that the DEBT is discharged -- the defect is root-caused by frame (c1), has a regression test that exercises the losing writer (c2), and has not reproduced in 80+ gradings across three commits -- it does not assert that a hosted-runner rate was ever taken. WHAT WOULD FALSIFY THIS: either entry reappearing in `.config/flaky-allowlist.txt`, or a `<flakyFailure>` for either test on a Windows CI leg, which now reds `report` directly because there is no row left to allow it. That is the intended consequence of the deletion and is the reason `absent:` is the anchor here rather than `commit:` -- a merge that restored the lines would silently re-open the debt, and `absent:` re-reads the file on every run."
  - id: c4
    text: "NOT MEASURED, and recorded as such: the rate on a hosted Windows runner."
    state: met
    evidence: "file:crates/wcore-config/src/credentials.rs:6972:hosted-runner full-suite contention"
    owner: core
    note: "STILL MET, and RE-ANCHORED rather than left to rot. This criterion asks for the hosted-Windows-runner rate to be recorded as NOT MEASURED, and it still is -- but the row it used to be anchored on (`.config/flaky-allowlist.txt:87`) was DELETED by c3 in this same change, which is exactly what c3 asks for. A criterion whose whole content is a record cannot be anchored on a line another criterion is required to delete; that tension was built into this pair and it is resolved here by MOVING THE RECORD, not by dropping it. It now lives in the `chunk_write_lock_verification` module doc block in `crates/wcore-config/src/credentials.rs`, beside the tests it is about, and it carries the same three things the allowlist row did: the negative control (SeanDesktop, `--retries 0`, n=15, 0 failures, 3.240-4.412s each), the reason that control is REFUSED as a rate (a fast box with an interactive session cannot reproduce hosted-runner full-suite contention), and the statement that a real rate needs the failing environment. It also now records the 50 further gradings from c2 and the 5 full-suite `--retries 0` runs from c3, all of which are the SAME box and therefore extend the control without changing what it can prove. WHAT WOULD FALSIFY THIS: the paragraph being deleted from the module doc, or a hosted-runner rate being measured somewhere without this row being re-graded. The source is a better home than the allowlist ever was: an allowlist row is debt that is designed to be deleted, and this record has to outlive the debt."
---

# The test name says splicing. The payload said the write failed. It was the lock.

`racing_writers_never_yield_a_spliced_credential` did not observe a spliced
credential. One of its two concurrent `chunked_put` calls returned
`Io(Os { code: 5, kind: PermissionDenied })` in 0.164 s, and the splicing
assertion never ran.

The frame is `ExclusiveFileLock::acquire`, and nothing else in that test could
have produced it: the store is an in-memory `HashMap`, so the manifest publish
never reaches a filesystem, and every other file operation on the path is
discarded with `let _ =`. On Windows, `DeleteFile` against a lockfile whose last
handle has not closed leaves the NAME in the directory in a delete-pending
state, and until it closes every `CreateFile` against that name — `CREATE_NEW`
included — is answered `ERROR_ACCESS_DENIED` (5), never `ERROR_FILE_EXISTS`.
The loop special-cased only `AlreadyExists`, so the one arm that handles
contention could not see the contention.

The stake is in the test's own doc block: ChatGPT refresh tokens rotate and are
single-use. A write that answers *Access is denied* after the provider has
already rotated the token burns it server-side and does not land, and the user
re-authenticates. Two Wayland processes refreshing one provider is the ordinary
way to get there.

`ERROR_ACCESS_DENIED` also means what it says, and the two states are
indistinguishable at the error-code level — measured on Windows 11 build 26200,
a delete-pending name and a directory under a real deny-ace return the same
`kind=PermissionDenied raw=Some(5) "Access is denied. (os error 5)"`. What
separates them is that delete-pending attaches to ONE name: during it, a
different name in the same directory is still accepted (`true`), while under the
deny-ace no name is (`false`). That probe, plus a 2 s grace on how long one
refusal may persist, is the discriminator; either observation alone reports the
denial unchanged.

Deliberately NOT folded into wayland#1300. That one is a 180 s harness kill in
`chunk_crash_injection` whose assertion also never runs, but its mechanism is a
48x bistable recovery path. Same module family, different shape; collapsing
them would hide one.
