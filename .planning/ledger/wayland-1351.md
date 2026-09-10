---
issue: 1351
repo: FerroxLabs/wayland
kind: defect
title: "Two sessions racing a memory migration both degrade to NullMemory, and the user is never told"
status: open
last_verified_commit: 4ca54eef7
criteria:
  - id: c1
    text: "Two sessions starting concurrently against a memory database that needs a migration BOTH end with a working memory backend rather than one of them on NullMemory -- shown by a test that reproduces the race and REDS without the fix."
    state: not-met
    owner: core
    note: "NOT MET, filed 2026-09-10 by this cycle's own verification while fixing the shared-home test isolation, and given a ledger IMMEDIATELY rather than being left to be found late -- that is the exact failure wayland#1272 c2 exists to catch, and filing a defect at the tail without tracking it would repeat it. THE RACE, measured rather than reasoned: two processes both read `user_version` below target, both run the same `ALTER TABLE`, and the loser gets `Migration { version: 5, source: SqliteFailure(..., Some("duplicate column name: last_latency_ms")) }`. `Memory::open` then returns Err and bootstrap falls back to NullMemory at crates/wcore-agent/src/bootstrap.rs:2231-2238. Observed on hetzner under tools/remote-proof.py, which exports ONE WAYLAND_HOME for the whole cargo invocation, so every process it spawns resolves the same memory database. WHY IT MATTERS FOR THIS RELEASE SPECIFICALLY: the race needs a pending migration plus concurrent starts, which is precisely the shape of the first minutes after an upgrade -- a user who starts two agents after installing 0.13.14 can lose long-term memory in one of them. WHAT WOULD SATISFY THIS: the loser must RETRY rather than degrade -- re-read `user_version` and re-open once when the failure is an already-applied migration. A NEW LOCKING SCHEME IS NOT THE PREFERRED FIX and should not be reached for at the tail of a release: this repo has already been bitten by a fork-duplicated flock on the session-journal data file, where 47.6% of reopens were refused under load and an agent blocked behind its own child. Retry is the smaller change and does not add a new lock to a release window."
  - id: c2
    text: "A session that does end on NullMemory tells the user through a channel that reaches them with RUST_LOG unset. A `tracing::warn!` does not qualify -- with RUST_LOG unset only ERROR reaches stderr -- shown by an arm that captures the user-facing stream."
    state: not-met
    owner: core
    note: "NOT MET, filed 2026-09-10. This is the half that makes the defect a LIE rather than merely a bug. The only announcement of the degradation is `tracing::warn!` at crates/wcore-agent/src/bootstrap.rs:2232. With RUST_LOG unset -- the default for every ordinary user -- ONLY ERROR reaches stderr, so that line is invisible and the user is never told their memory stopped working. NullMemory accepts every write and returns a fresh id, so nothing downstream looks wrong either. WHAT IS ALREADY CLOSED, and should not be re-litigated: the F05 fail-closed path at bootstrap.rs:2243-2249 sets smart_handoff_to_memory = false and skills_lifecycle = false when memory is unconstructed, precisely so runtime code cannot falsely emit outcome_changed. That stops the product misreporting a RESULT. It does not tell the user the FEATURE is off, which is what this criterion is about. GRADE IT ON A CAPTURED STREAM, NOT ON A CODE READING: an arm that runs the degraded path with RUST_LOG unset and shows the user-facing stream is empty is the red; the same arm showing the disclosure after the fix is the green. A log-level bump alone has already failed to fix `the user is not told` three times in this repo, so a warn! -> error! change is not automatically sufficient -- what matters is that it reaches the user's stream under the default configuration."
---

Created 2026-09-10 at filing time, not retroactively. Found by the w15/lease
lane while isolating the shared-home test failures; the memory race was the
common cause behind several unrelated-looking assertion failures.
