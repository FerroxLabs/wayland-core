# THE BLOCKER IS CLOSED — wayland-core RC — 2026-08-05

Branch `plan/f20-unified-audit-repair`. Workspace 0.12.26. Nothing tagged.

Supersedes `HANDOFF-2026-08-05-THE-PROBLEM.md`, which named one blocker and
proposed the wrong fix for it. Both corrections are recorded below rather than
quietly replaced.

---

## 1. WHAT THE BLOCKER ACTUALLY WAS

The prior handoff said: *a Windows worktree cleanup fails with
`ERROR_SHARING_VIOLATION` and the code never retries.* That was half of it, and
the proposed repair — a bounded retry at `dispatch.rs:623` — was the wrong
place. Measurement changed both conclusions.

### There were TWO defects, not one

| # | defect | measured rate |
|---|---|---|
| 1 | **The workspace-accounting walk demanded `DELETE` on a read-only scan**, so a live `git` inside `.git` killed a HEALTHY worker with `workspace accounting refused ".git": ... (os error 32)` | 8 of 24 runs |
| 2 | **Destructive cleanup had no patience** for a sub-10 ms handle transient it cannot wait out | 8 of 8 refusals, all cleared at 10 ms |

Defect 1 is a **product defect**, not a test flake. On Windows, any operator
whose swarm checkout has a live process standing in it — a `git` child, an
editor, a shell — could have a healthy worker killed and be told the wrong
reason. It was invisible until the reproducer separated the two failure
strings, because both land on the same assertion (`worker_runtime_limits.rs:55`).

### Three theories the measurement refuted

1. **"An AV scanner or the search indexer holds the handle."** Unsupported. The
   holder for defect 1 is our own accounting walk asking for a right it never
   uses.
2. **"The job drain fails open — either the 5 s deadline or a failed
   `QueryInformationJobObject`."** Both refuted directly: instrumented across
   **147 observations**, every single drain exited at `ActiveProcesses == 0` in
   **0 ms**. Zero deadline expiries, zero query failures.
3. **"Retry at `dispatch.rs:623`."** Actively unsafe. By then the error is
   flattened to `WorktreeIo(String)` and the errno is gone, so the retry could
   only match a LOCALIZED message. Worse, `release()` closes the transaction
   lease inside the swarm critical section and drops the sentinel on return, so
   between two attempts up there the root is observably lease-free with its
   reservation receipt on disk — exactly what `reclaim_abandoned_transactions`
   treats as abandoned, and the in-process registry does not exclude a peer
   PROCESS. Retrying there reopens a cross-process reclaim window that ordering
   was written to close.

---

## 2. THE FIX — commit `d77fe972`

1. **`logical_tree_bytes` now uses `open_child_directory_observational`.** It
   only enumerates names and reads lengths; two callers already described it in
   their own comments as "read-only enumeration" while the code demanded
   `FILE_GENERIC_WRITE | DELETE`. `DELETE` is share-arbitrated on Windows.
2. **`remove_descendants` retries the delete-bearing open**, bounded to seven
   attempts over ~785 ms, and after the budget is spent returns the original
   refusal unchanged.

Retryability is proven **positively by errno**: only `SandboxError::Io` carries
a typed `io::Error`, and only `raw_os_error() == Some(32)` qualifies. Every
security refusal on that path is a different variant (`PathDenied`,
`ExecFailed`), so a denial can never be retried by accident. No string matching
— `io::Error`'s Display is localized.

---

## 3. EVIDENCE

| check | result |
|---|---|
| Windows unit tests (new) | 5/5 pass |
| Windows clippy `-D warnings` | clean |
| Linux `wcore-swarm` + `wcore-sandbox` | 263/263 pass, clippy clean |
| Windows live reproducer | see §4 |

The observational test carries its **own positive control**: it asserts in the
same test that the mutating open IS refused while the pin is held, so it cannot
silently degrade into a test that proves nothing. The retry tests pin the bound
(`len(schedule) + 1` attempts), prove a `PathDenied` is never re-attempted, and
prove errno 5 is not treated as errno 32.

---

## 4. HOW TO RE-RUN THE REPRODUCER

The prior handoff's harness advice was wrong in two ways worth keeping:

- **32 CPU burners is too harsh.** It trips the test's own 20 s wall-clock
  assert at `worker_runtime_limits.rs:48` before cleanup is ever reached — a
  different failure that poisons the measurement.
- **`--test-threads=1` stack-overflows** (0xC00000FD): libtest then runs on the
  main thread's 1 MB Windows stack. Set `RUST_MIN_STACK` and let libtest spawn.

What works is **concurrent test PROCESSES**, which is what nextest actually
does. Scripts live in this session's scratchpad (`ps/verify.ps1`).

Base rate at concurrency 4, no burners: **~33%** — far above the 1-in-16 the
prior handoff assumed. At that rate 100 trials is overwhelming evidence, so the
"several hundred iterations" advice was calibrated to a rate that was wrong.

---

## 5. STILL OPEN — neither blocks the RC, both recorded not hidden

1. **Two sibling read-walks still demand `DELETE`**:
   `worktree/candidate.rs:462` and `worktree/parent.rs:1415`. Same defect class
   as fix 1. Deliberately excluded from rc.1 because they are UNMEASURED, and
   measure-then-fix is the discipline that worked today. Measure under
   contention first.
2. **`mark_open_object_for_delete` embeds the OS error as TEXT** in
   `SandboxError::ExecFailed`, so a sharing violation landing on the delete
   disposition rather than the open is invisible to the errno-gated retry.
   Never observed in any instrumented run; left untyped rather than widened
   speculatively.

---

## 6. CI

Run 30971991803 (head `d06935d5`) finished: **Linux success, macOS success,
Windows the only failure.** macOS was the leg genuinely in doubt and it is
green, so the heartbeat fix and the lock-test rescale both hold.

Next run must be over `d77fe972`. **Do not push while a run is in flight** — it
cancels it.

## 7. THE PATH TO A TAG

1. One green CI run over `d77fe972`.
2. Tag `v0.12.26-rc.1`.

Tagging is reserved to Sean and authorised only over genuinely green CI.
