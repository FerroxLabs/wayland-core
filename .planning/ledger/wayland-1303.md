---
issue: 1303
repo: FerroxLabs/wayland
kind: defect
title: "Windows: a racing chunked credential write fails outright with ACCESS_DENIED (os error 5), and the caller loses a single-use refresh token"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "The Windows file operation that answers ERROR_ACCESS_DENIED is identified BY FRAME rather than inferred, and the answer distinguishes lock acquisition from manifest publish."
    state: not-met
    owner: core
    note: "The artifact gives the caller frame and not the callee: credentials.rs:6133:40 is `handle.join().unwrap().expect('both writers must succeed')`, so all that is established is that one of two concurrent chunked_put calls returned Err(Io(Os { code: 5 })). ERROR_ACCESS_DENIED is what a delete-pending handle answers on Windows -- the sibling of the ERROR_SHARING_VIOLATION class this repo has already met -- which makes the lock acquire and the manifest publish the two candidates. This criterion exists so the ticket cannot be graded met on that story."
  - id: c2
    text: "A concurrent chunked_put on Windows never returns Err for a race it is supposed to serialise: a regression test exercises the LOSING writer and asserts it either commits whole or retries, measured on Windows at --retries 0."
    state: not-met
    owner: core
    note: "The existing tests assert the committed VALUE is one whole token set; neither asserts what the loser is entitled to. That gap is why a hard Err reads as a flake -- the test fails in its setup expectation rather than on its invariant, and a retry hides it."
  - id: c3
    text: "Both chunk_write_lock_verification tests come off .config/flaky-allowlist.txt, and the entries are DELETED rather than renewed."
    state: not-met
    owner: core
    note: "Listed 2026-09-03 with a 2026-09-20 expiry so the debt cannot quietly roll forward. Allowlisting was chosen over a scoped `retries = 0` only because a ten-PR train was blocked on a pre-existing Windows reliability bug; the allowlist header is explicit that retries = 0 is the correct home once the train is clear."
  - id: c4
    text: "NOT MEASURED, and recorded as such: the rate on a hosted Windows runner."
    state: not-met
    owner: core
    note: "Negative control on SeanDesktop (the ferrox-win-msvc host) at --retries 0, n=15, 0 failures, 3.240-4.412s each, genuinely executing. That box is fast and has an interactive session, so it does not reproduce hosted-runner full-suite contention; the green is not evidence against the defect and must not be cited as one. A real rate needs the failing environment."
---

# The test name says splicing. The payload says the write failed.

`racing_writers_never_yield_a_spliced_credential` did not observe a spliced
credential. One of its two concurrent `chunked_put` calls returned
`Io(Os { code: 5, kind: PermissionDenied })` in 0.164 s, and the splicing
assertion never ran.

The stake is in the test's own doc block: ChatGPT refresh tokens rotate and are
single-use. A write that answers *Access is denied* after the provider has
already rotated the token burns it server-side and does not land, and the user
re-authenticates. Two Wayland processes refreshing one provider is the ordinary
way to get there.

Deliberately NOT folded into wayland#1300. That one is a 180 s harness kill in
`chunk_crash_injection` whose assertion also never runs, but its mechanism is a
48x bistable recovery path. Same module family, different shape; collapsing
them would hide one.
