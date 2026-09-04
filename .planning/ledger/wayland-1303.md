---
issue: 1303
repo: FerroxLabs/wayland
kind: defect
title: "Windows: a racing chunked credential write fails outright with ACCESS_DENIED (os error 5), and the caller loses a single-use refresh token"
status: open
last_verified_commit: c1d4d77b8
criteria:
  - id: c1
    text: "The Windows file operation that answers ERROR_ACCESS_DENIED is identified BY FRAME rather than inferred, and the answer distinguishes lock acquisition from manifest publish."
    state: met
    evidence: "symbol:crates/wcore-config/src/credentials.rs::acquire_with"
    owner: core
    note: "MET at c1d4d77b8. THE FRAME IS THE LOCK ACQUISITION, not the manifest publish, and it is settled by ENUMERATION rather than by the delete-pending story. In the failing test the store is `Shared { entries: Mutex<HashMap<String, String>> }` -- an in-memory map -- so no manifest publish touches a filesystem at all and cannot produce an io::Error under any schedule. Every OTHER filesystem call reachable from chunked_put there is discarded and cannot propagate: is_stale's metadata (`.unwrap_or(false)`), the stale-steal remove_file, and Drop's read_to_string/remove_file (all `let _ =`). secure_credential_dir is create_dir_all plus a `#[cfg(unix)]` chmod, and create_dir_all on an existing directory returns Ok. That leaves exactly ONE propagated call: the `create_new(true).write(true).open(path)` inside the acquisition loop, whose PermissionDenied answer fell through to `Err(e) => return Err(CredentialsError::Io(e))`. MEASURED ON REAL WINDOWS, not inferred: on Windows 11 build 26200, a lockfile put into delete-pending (FileDispositionInfo with the handle still open) answers CREATE_NEW with `kind=PermissionDenied raw=Some(5) msg=Access is denied. (os error 5)` -- byte-identical to the CI payload -- and `is it the AlreadyExists the lock special-cased? false`, which is why the one contention arm missed it. Once the last handle closes the same CREATE_NEW returns Ok, so the state is transient. WHAT WOULD FALSIFY THIS: a payload from the same test whose io::Error can be traced to a store that touches the filesystem, or a Windows measurement in which a delete-pending name answers CREATE_NEW with ERROR_FILE_EXISTS."
  - id: c2
    text: "A concurrent chunked_put on Windows never returns Err for a race it is supposed to serialise: a regression test exercises the LOSING writer and asserts it either commits whole or retries, measured on Windows at --retries 0."
    state: not-met
    owner: core
    note: "HALF DONE, AND THE MISSING HALF IS THE MEASUREMENT. The regression test now exists and is the losing writer end to end: chunk_write_lock_verification::a_losing_writer_denied_by_a_delete_pending_lockfile_still_commits_whole scripts the loser's own lockfile create to answer `Access is denied. (os error 5)` for the delete-pending window and then asserts the loser committed its WHOLE token set. It is a real red: with only the classification arm reverted it fails with `Io(Custom { kind: PermissionDenied, error: \"Access is denied. (os error 5)\" })` out of chunked_put, which is the ticket's payload. THE CLAUSE THIS ROW STILL FAILS is `measured on Windows at --retries 0`. It was NOT run on Windows. The only Windows host reachable from this lane is SeanDesktop, and c4 already records why that host's green is refused as evidence about this defect: it is fast, interactive, and cannot reproduce hosted-runner full-suite contention -- it scored n=15, 0 failures against the UNFIXED code. A green there would be a green from a host that cannot exhibit the failure. What WAS measured on real Windows (11, build 26200) is the algorithm, standalone: a waiter running this loop against a genuinely delete-pending lockfile held for 400ms ACQUIRED after 8 retries in 403.8ms instead of returning Err. That supports c1 and the discriminator; it is not the suite run this row asks for. CLOSE THIS ROW with a hosted-runner Windows leg at --retries 0, not with another SeanDesktop run."
  - id: c3
    text: "Both chunk_write_lock_verification tests come off .config/flaky-allowlist.txt, and the entries are DELETED rather than renewed."
    state: not-met
    owner: core
    note: "NOT DONE, AND DELIBERATELY NOT DONE. The two entries at .config/flaky-allowlist.txt:87-88 are untouched. Removing them now would assert that the Windows flake is closed on the strength of a fix that has never run on Windows CI -- exactly the fail-open shape c2 is still open on. The order is: land the fix, get one hosted-runner Windows leg at --retries 0 (c2), then delete both lines in the same change. WHAT WOULD FALSIFY THIS ROW BEING HONEST: the entries disappearing while c2 is still not-met, or their 2026-09-20 expiry being renewed instead of the lines being deleted."
  - id: c4
    text: "NOT MEASURED, and recorded as such: the rate on a hosted Windows runner."
    state: met
    evidence: "file:.config/flaky-allowlist.txt:87:so this proves it flakes and NOT how often"
    owner: core
    note: "MET at 509f4426b BY RECORD, and STILL MET at c1d4d77b8: the allowlist row is untouched by this change. This criterion asks for the rate to be recorded as NOT MEASURED, and it is, together with the negative control and the reason that control does not count as evidence against the defect: SeanDesktop (the ferrox-win-msvc host) at --retries 0, n=15, 0 failures, 3.240-4.412s each and genuinely executing -- but that box is fast and has an interactive session, so it cannot reproduce hosted-runner full-suite contention, and its green is explicitly refused as counter-evidence. A real rate needs the failing environment. WHAT WOULD FALSIFY THIS: a rate being measured and this row not being re-graded, or the record being deleted. ANCHORED ON THE ALLOWLIST ROW, not on this file: .config/flaky-allowlist.txt:87 carries the same record where the gate reads it -- the n=15 control, and the sentence that refuses it as a rate. A self-anchor into this ledger is structurally impossible under the file: grammar, because the token text lands in the file it points at and the fragment then matches twice."
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
