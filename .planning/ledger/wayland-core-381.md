---
issue: 381
repo: FerroxLabs/wayland-core
kind: defect
title: "The @dir walk blocks forever on a FIFO in the workspace, wedging the turn with no cancellation path"
status: closed
last_verified_commit: b437de07
criteria:
  - id: c1
    text: "@./ in a workspace containing a named pipe completes rather than blocking"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::the_at_dir_walk_does_not_block_on_a_fifo"
    owner: core
    note: "CLOSED 2026-08-30 (lane f13-atref-guard-b), alongside FerroxLabs/wayland-core#339 c2, which is where this defect was found. REPRODUCED FIRST at origin/integ/f13 a278f8c3b: the new test was run against the unfixed walk and went red with 'the @dir walk BLOCKED for 10s on a FIFO in the workspace - the turn is wedged, not slow' (the 10s is the test's timeout; the block is unbounded). FIXED AT BOTH UNGUARDED SITES, not just the reported one: walk_dir stats with fs::metadata - which follows the link and only stats, so it cannot block - and refuses anything that is not a regular file BEFORE admit() opens it; and read_guarded, the @symbol preview's read site, refuses a non-regular path the same way. resolve_file already had the filter, so that is all three read sites on this surface. RED ARM, verbatim, mutating each filter ALONE so neither test is carried by the other (both diff-verified to land on the `if`, not a comment): `if !meta.is_file() {` -> `if false {` gave 'panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:1382:23: the @dir walk BLOCKED for 10s on a FIFO in the workspace - the turn is wedged, not slow', and `if !path.is_file() {` -> `if false {` gave 'panicked at crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:1406:23: read_guarded BLOCKED for 10s opening a FIFO - the same wedge as the @dir walk'. Restored and re-run: 'test result: ok. 73 passed; 0 failed'. ORIGINAL FILING: Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D7, found while verifying wayland-core#339 — SECURITY: the @-ref secret guard is lexical, so a symlink bypasses it). Nothing has been done. The measured finding, verbatim: The @dir walk blocks forever on a FIFO in the workspace. `walk_dir` treats every non-directory entry as readable — it has no `is_file()` filter, unlike `resolve_file`, which does guard with `if !full.is_file() { NotFound }`. `admit()` therefore calls `same_file::Handle::from_path` (a plain `File::open`) on a named pipe, which blocks until a writer appears. REPRODUCED, not modelled: a probe test that `mkfifo`s `pipe` next to `ok.txt` and calls `resolve(@./, root)` on a worker thread panicked with 'PROBE_FIFO the @dir walk BLOCKED for 10s on a FIFO in the workspace' (the 10s is my timeout, the block is unbounded). A control in the same run passed, so the harness works."
  - id: c2
    text: "walk_dir skips any entry that is not a regular file BEFORE admit() opens it"
    state: met
    evidence: "symbol:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::walk_dir"
    owner: core
    note: "CLOSED 2026-08-30 (lane f13-atref-guard-b). The `let Ok(meta) = fs::metadata(&path)` / `if !meta.is_file() { *skipped += 1; continue; }` pair sits in walk_dir's else branch immediately ABOVE the `admit(&path, &path)` that opens the entry - the ordering is the whole point, since admit() is the call that blocks. Red arm on c1. ORIGINAL FILING: Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D7). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test plants a FIFO next to an ordinary file, calls resolve(@./, root) under a timeout, and fails if the call does not return; shown RED against today's code"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::the_at_dir_walk_does_not_block_on_a_fifo"
    owner: core
    note: "CLOSED 2026-08-30 (lane f13-atref-guard-b). Literally that shape: mkfifo(pipe) next to ok.txt, resolve(@./, root) on a worker thread, recv_timeout(10s), panic on timeout. Shown red against origin/integ/f13 a278f8c3b - verbatim output on c1. The sibling site read_guarded has its own timed test, read_guarded_does_not_block_on_a_fifo. ORIGINAL FILING: Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D7). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "Controls stay green: an ordinary file in the same directory is still attached, and a FIFO named .env is still not read"
    state: met
    evidence: "test:crates/wcore-cli/src/tui/commands/at_ref_resolve.rs::the_at_dir_walk_does_not_block_on_a_fifo"
    owner: core
    note: "CLOSED 2026-08-30 (lane f13-atref-guard-b). BOTH controls are asserted inside the same test, so it cannot be satisfied by a walk that skips everything: ok.txt beside the FIFO must still be attached, and a SECOND fifo named .env must not appear in the payload. The .env arm is the one this issue calls out specifically, because core#339 widened the wedge there - admit() now runs BEFORE is_secret_path, so a FIFO named .env blocked on the open before its name was ever judged, where the pre-core#339 walk skipped it by name without opening it. ORIGINAL FILING: Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D7). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The @dir walk blocks forever on a FIFO in the workspace. `walk_dir` treats every non-directory entry as readable — it has no `is_file()` filter, unlike `resolve_file`, which does guard with `if !full.is_file() { NotFound }`. `admit()` therefore calls `same_file::Handle::from_path` (a plain `File::open`) on a named pipe, which blocks until a writer appears. REPRODUCED, not modelled: a probe test that `mkfifo`s `pipe` next to `ok.txt` and calls `resolve(@./, root)` on a worker thread panicked with 'PROBE_FIFO the @dir walk BLOCKED for 10s on a FIFO in the workspace' (the 10s is my timeout, the block is unbounded). A control in the same run passed, so the harness works.

**Where.** crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:397 (the `admit(&path, &path)` in walk_dir's else branch) — and the pre-existing form of the same bug at the `fs::read_to_string(&path)` it replaced. Reached from engine_bridge.rs:1717 -> at_refs::resolve_message_with.

**Why it matters.** A user who types `@./` in any workspace containing a named pipe hangs their turn permanently. It happens on the spawned turn task inside a blocking syscall, before the engine loop is entered, so it is not an .await point and turn cancellation cannot reach it — the session is wedged, which is the same user-visible shape as the Esc-wedge class. The trigger is ordinary: build systems, editors and language servers all leave FIFOs and sockets in trees. This is PRE-EXISTING (verified: `git show 6d130a62^` shows the pre-fix walk calling `fs::read_to_string(&path)` with the same absent file-type filter), but nobody has filed it. #339 also slightly widened it: `admit()` now runs BEFORE `is_secret_path`, so a FIFO named `.env` — which the pre-fix walk skipped by name without ever opening it — now blocks too. Fix is one line: skip any entry whose `symlink_metadata`/`metadata` says it is not a regular file, before `admit()`.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
