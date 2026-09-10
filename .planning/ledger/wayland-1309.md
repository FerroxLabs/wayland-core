---
issue: 1309
repo: FerroxLabs/wayland
kind: defect
title: "raw_mode_with_nothing_typed_still_denies: the pty capture ends at the prompt, so a missing denial reason and a truncated read are indistinguishable"
status: open
last_verified_commit: 676336306
criteria:
  - id: c1
    text: "The failure distinguishes the two readings: the assertion waits for a terminal condition rather than sampling whatever the pty holds at one moment, and on failure reports how long it waited."
    state: met
    evidence: "symbol:crates/wcore-agent/tests/approval_pty_raw_partial_line.rs::DRAIN_BUDGET"
    owner: core
    note: "MET at 7e0a105a4. The assertion no longer samples the pty: the reader thread announces the TERMINAL condition it reached (EOF, or the EIO Linux returns once the last slave dup closes) AFTER its final push into the transcript, and `run_arm` waits for that announcement -- bounded by DRAIN_BUDGET, 5s, past the reaped child -- before cloning it. The two readings are then reported APART: a reader still blocked when the budget runs out fails FIRST with `CAPTURE INCOMPLETE -- this grades the harness, not the product`, quoting the child outcome, its elapsed and the drain wait; only a capture the reader took to a terminal state can reach the reason assertion, which says `THE CAPTURE IS COMPLETE` and names the terminal condition and how long it waited. THE INSTRUMENT IMMEDIATELY FOUND A REAL DEFECT IN ITSELF: `openpty` leaves FD_CLOEXEC clear and these arms run in parallel, so every arm`s pty was inherited by every LATER arm`s child. MEASURED at 12f9d1094 on hetzner-dsm: the two arms whose child answers at once reaped it after 0.050s and then waited a further 1.952s for the master to go terminal -- 2.002s in total, exactly when the two 2s-budget arms` children exited and released the slave they had inherited. That is the mechanism by which a child exit did not mean the transcript was complete. With CLOEXEC set at f0aba0e5e the same arms measure drain_wait=0.000s. LIMIT: the historical capture of run 33752019921 cannot be re-graded; what is closed is that the ambiguity can no longer be produced."
  - id: c2
    text: "With the denial-reason write deliberately suppressed, the test still reds."
    state: met
    evidence: "commit:dd84ade27"
    owner: core
    note: "MET at 7e0a105a4, and this is the criterion that separates a fix from a silencing. RED ARM at commit b41d38397: the `eprintln!` in the `AnswerRead::Expired` match arm of `crates/wcore-agent/src/confirm.rs` replaced by a discarded `format!` -- the verdict still Denied, the reason never written. The mutation was PRINTED IN CONTEXT before it was believed and lands on executable code inside a match arm, not on a comment. ARMS on hetzner-dsm in proof slot parallel-1, `cargo test -p wcore-agent --test approval_pty_raw_partial_line`: MUTATED at b41d38397 -- 4 passed, 1 FAILED, `raw_mode_with_nothing_typed_still_denies` at approval_pty_raw_partial_line.rs:387 with `the operator must be told why it was denied. THE CAPTURE IS COMPLETE: the pty reader reached Terminal(Input/output error (os error 5)) 0.000s after the child was reaped ... the reason is genuinely ABSENT rather than merely unread`, transcript_bytes 203 -> 83. It failed on the COMPLETE-CAPTURE assertion, not the truncation one, which is the whole point of c1. RESTORED at 7e0d5d3e6 -- `git diff dcd9ff79a` for confirm.rs is EMPTY, the file was touched after restore so cargo could not skip the rebuild -- and the arm is green again. PASS-AFTER at f0aba0e5e and at 7e0a105a4: 5 passed, 0 failed."
  - id: c3
    text: "Measured on Linux at --retries 0, n>=20, under full-workspace contention rather than alone."
    state: met
    evidence: "file:.planning/ledger/wayland-1309.md"
    owner: core
    note: "MET at 7e0a105a4. MEASURED, and measured with the whole workspace running rather than the file alone -- which is the point, because contention is what decides whether a read lands early. INSTRUMENT: `cargo nextest run --workspace --profile ci --retries 0 --no-fail-fast` on hetzner-dsm in proof slot parallel-1, i.e. 18,094 tests across 871 binaries in one parallel pool on a 96-core box, so the four arms run against the full population and not by themselves; the arms land around position 3,000-4,100 of 18,094, with the pool saturated. N = 21 COMPLETE RUNS at this exact source sha, each with a `complete: true` proof receipt (source, tree and remote exit all verified), tree unchanged between them. RESULT: 5 of 5 arms PASS in every one of the 21 runs; ZERO FAIL, TRY, SIGSEGV or TIMEOUT lines for this binary across all 21. 0/21 at retries 0 bounds the rate near 13 percent upper and does NOT refute a low single-digit one -- the original was observed ONCE, in run 33752019921, and the ledger recorded the rate as unmeasured. What 0/21 under contention does refute is the reading that the reason was never written: with the reason suppressed the same instrument reds on the first attempt (c2). STATED PLAINLY: this is an absence over 21 runs, and an absence is weaker evidence than the red arm in c2. c2 is what makes this criterion mean anything."
  - id: c4
    text: "The entry comes off .config/flaky-allowlist.txt and is DELETED rather than renewed."
    state: met
    evidence: "absent:.config/flaky-allowlist.txt::raw_mode_with_nothing_typed_still_denies"
    owner: core
    note: "MET at 7e0a105a4. DELETED, not renewed and not re-dated: the entry that stood at .config/flaky-allowlist.txt:104 with a 2026-09-20 expiry is gone, and no entry replaces it. The evidence token needles the TEST NAME rather than the issue tag -- the #1182 lesson, where a needle one string wide let the same test be re-listed under another owner with the ledger still green -- so a resurrection under any tag reds this criterion on the next gate run. The absence is controlled: the file still exists and still carries its other dated entries, so an empty result is the entry being gone rather than the query being broken. It comes off because c1, c2 and c3 are closed: the ambiguity the entry deliberately refused to resolve is resolved, and it was resolved in the direction the entry warned against assuming -- a HARNESS defect (a detached reader raced by a snapshot, and a pty inherited across parallel arms), with the product`s reason write proven live by suppressing it and watching the test red."
---

# Filed from the fold-in run, not the original outage window

Bringing wayland-core#433 into wayland-core#432 produced a new tree, and a new
tree is a new sample. The retry-flake gate can only ever report whichever
member fires, so this cluster was invisible until then.
