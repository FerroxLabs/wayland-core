---
issue: 381
repo: FerroxLabs/wayland-core
kind: defect
title: "The @dir walk blocks forever on a FIFO in the workspace, wedging the turn with no cancellation path"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "@./ in a workspace containing a named pipe completes rather than blocking"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D7, found while verifying wayland-core#339 — SECURITY: the @-ref secret guard is lexical, so a symlink bypasses it). Nothing has been done. The measured finding, verbatim: The @dir walk blocks forever on a FIFO in the workspace. `walk_dir` treats every non-directory entry as readable — it has no `is_file()` filter, unlike `resolve_file`, which does guard with `if !full.is_file() { NotFound }`. `admit()` therefore calls `same_file::Handle::from_path` (a plain `File::open`) on a named pipe, which blocks until a writer appears. REPRODUCED, not modelled: a probe test that `mkfifo`s `pipe` next to `ok.txt` and calls `resolve(@./, root)` on a worker thread panicked with 'PROBE_FIFO the @dir walk BLOCKED for 10s on a FIFO in the workspace' (the 10s is my timeout, the block is unbounded). A control in the same run passed, so the harness works."
  - id: c2
    text: "walk_dir skips any entry that is not a regular file BEFORE admit() opens it"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D7). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c3
    text: "A test plants a FIFO next to an ordinary file, calls resolve(@./, root) under a timeout, and fails if the call does not return; shown RED against today's code"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D7). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
  - id: c4
    text: "Controls stay green: an ordinary file in the same directory is still attached, and a FIFO named .env is still not read"
    state: not-met
    owner: core
    note: "Filed 2026-08-29 by the 0.13.12 close-sweep (worklist defect D7). Nothing has been done; the measurement is on c1 and the file:line anchors are in the prose below."
---

The @dir walk blocks forever on a FIFO in the workspace. `walk_dir` treats every non-directory entry as readable — it has no `is_file()` filter, unlike `resolve_file`, which does guard with `if !full.is_file() { NotFound }`. `admit()` therefore calls `same_file::Handle::from_path` (a plain `File::open`) on a named pipe, which blocks until a writer appears. REPRODUCED, not modelled: a probe test that `mkfifo`s `pipe` next to `ok.txt` and calls `resolve(@./, root)` on a worker thread panicked with 'PROBE_FIFO the @dir walk BLOCKED for 10s on a FIFO in the workspace' (the 10s is my timeout, the block is unbounded). A control in the same run passed, so the harness works.

**Where.** crates/wcore-cli/src/tui/commands/at_ref_resolve.rs:397 (the `admit(&path, &path)` in walk_dir's else branch) — and the pre-existing form of the same bug at the `fs::read_to_string(&path)` it replaced. Reached from engine_bridge.rs:1717 -> at_refs::resolve_message_with.

**Why it matters.** A user who types `@./` in any workspace containing a named pipe hangs their turn permanently. It happens on the spawned turn task inside a blocking syscall, before the engine loop is entered, so it is not an .await point and turn cancellation cannot reach it — the session is wedged, which is the same user-visible shape as the Esc-wedge class. The trigger is ordinary: build systems, editors and language servers all leave FIFOs and sockets in trees. This is PRE-EXISTING (verified: `git show 6d130a62^` shows the pre-fix walk calling `fs::read_to_string(&path)` with the same absent file-type filter), but nobody has filed it. #339 also slightly widened it: `admit()` now runs BEFORE `is_secret_path`, so a FIFO named `.env` — which the pre-fix walk skipped by name without ever opening it — now blocks too. Fix is one line: skip any entry whose `symlink_metadata`/`metadata` says it is not a regular file, before `admit()`.

Criteria are taken verbatim from the issue's Acceptance section. Nothing has been done: this entry exists so the release gate counts the work rather than anyone having to remember it.
