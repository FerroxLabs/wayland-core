---
phase: 20-transactional-delegated-mutation
plan: "73"
subsystem: swarm-native-windows
tags: [windows, locking, fd-lock, directory-authority, test-attribution]
status: complete
source_sha: 646ecda8185ce14e9521a175db4a9f4244b242cf
requires:
  - "20-72"
provides:
  - "wcore_sandbox::DirectoryAuthority::open_child_lock_file / open_or_create_child_lock_file"
  - "worktree::swarm_lock_handle / transaction_lease_handle"
  - "Windows mutual exclusion across every swarm transaction path"
affects:
  - crates/wcore-sandbox/src/directory_authority.rs
  - crates/wcore-sandbox/src/directory_authority_windows.rs
  - crates/wcore-swarm/src/worktree.rs
  - crates/wcore-swarm/src/worktree_security.rs
  - crates/wcore-swarm/src/worktree_manager.rs
  - crates/wcore-swarm/src/worktree_cleanup.rs
  - crates/wcore-swarm/src/worktree_tests.rs
  - crates/wcore-swarm/tests/dispatch_smoke.rs
tech-stack:
  added: []
  patterns:
    - "advisory locks target a REGULAR FILE resolved handle-relative, never a directory object"
    - "one derivation per semantic lock, so the interlock set cannot be split"
key-files:
  created: []
  modified:
    - crates/wcore-sandbox/src/directory_authority.rs
    - crates/wcore-sandbox/src/directory_authority_windows.rs
    - crates/wcore-swarm/src/worktree.rs
    - crates/wcore-swarm/src/worktree_security.rs
    - crates/wcore-swarm/src/worktree_manager.rs
    - crates/wcore-swarm/src/worktree_cleanup.rs
    - crates/wcore-swarm/src/worktree_tests.rs
    - crates/wcore-swarm/tests/dispatch_smoke.rs
decisions:
  - "Unix unifies onto the lock file too: one mechanism, one testable code path, no #[cfg] in wcore-swarm"
  - "The swarm sentinel lives inside CONTROL_DIR so all four swarm-root enumeration loops stay unchanged"
  - "B2 keeps its platform split (measured: the descendant reservation handle does refuse the rename)"
  - "B4 keeps its OS-refusal arms (measured: the repo rename IS refused under Swarm::new) and adds an out-of-repository coverage test"
metrics:
  duration: "~2h"
  completed: 2026-07-25
---

# Phase 20 Plan 73: Windows Advisory Lock Repair Summary

Retargeted every swarm advisory lock from a directory handle — on which `LockFileEx` is
undefined and returns error 87 unconditionally — to a regular-file sentinel resolved
handle-relative from the retained directory, and corrected four Windows test arms whose
rationales the measured rename truth table refutes.

## Root cause

Windows byte-range locking is UNDEFINED on directory objects. Measured on SEANDESKTOP
(Win11 10.0.26200.8875, NTFS): `LockFileEx(<directory HANDLE>, LOCKFILE_EXCLUSIVE_LOCK, ...)`
returns FALSE with `ERROR_INVALID_PARAMETER (87)` for every access mode and share mode
tried, while the same call on a regular-file handle succeeds. `fd-lock 4.0.4`
(`src/sys/windows/rw_lock.rs::write`) calls `LockFileEx` directly.

All three lock helpers in `worktree.rs` passed a directory handle, so **all eight of their
call sites failed closed on Windows since inception**: admission critical sections,
transaction leases, full cleanup and the active-transaction probe never mutually excluded
anything on that platform. This was a correctness/concurrency defect, not a cosmetic one.

## What now locks what

| Site | Old target | New target |
|---|---|---|
| `with_directory_lock` (3 callers) | swarm-root directory handle | `<swarm_root>/.wayland-control/.wayland-swarm-lock` |
| `cleanup_all` lease | swarm-root directory handle | same sentinel — admission/cleanup still serialize |
| both checkout registration leases | transaction-root directory handle | `<transaction_root>/.wayland-active-lease` |
| `transaction_is_active` (2 callers) | transaction-root directory handle | same lease file, open-only, non-mutating |

Exactly one `swarm_lock_handle` and one `transaction_lease_handle` derivation exist. The
redundant create-and-immediately-drop of the lease file is gone — `LEASE_FILE`'s name is
now accurate and `transaction_is_active` finally means what it says.

## Sentinel placement (load-bearing, not tidy)

The sentinel lives inside `CONTROL_DIR` because four production loops enumerate
`read_dir(&swarm_root)` and skip only the control directory —
`reserved_workspace_bytes`, `cleanup_all`, `retained_worker_count`, `sandbox_read_denies` —
and the first three hard-`Err` through `is_real_directory_entry` on any non-directory
child. A sentinel directly under the swarm root would break admission accounting, cleanup
and the residual sweep. Zero enumeration changes were needed.

## Loan accounting preserved exactly

The new primitive returns a `DirectoryHandleLoan` whose `loans` counter is the PARENT
directory's while its handle is the child lock file, so `remove_open_dir_all` still refuses
while a lease is held. Verified empirically: `PROBE[one file child]` and the lock-file
removal probe both succeed, so the lock file itself does not perturb removal.

## Unix unification decision and its migration consequence

Unix moved to the lock file too. A platform split would have put two different
mutual-exclusion mechanisms in the crate's most safety-critical primitive and made the
Windows mechanism unexercisable by any Linux-CI test — precisely the condition that hid
this defect for the project's entire life.

**Accepted consequence:** during a unix version-skew window, a process on an older build
(locking the directory) and one on this build (locking the sentinel) will NOT interlock.
Blast radius is bounded to admission serialization and aggregate capacity accounting. It
weakens NO authority proof: `validate_swarm_root`, `validate_repo_authority`, every
`DirectoryAuthority` identity check and the reservation receipts are lock-independent and
still fail closed. Windows has no compatibility surface to preserve — the lock never
engaged there at all. Recorded in-source at `SWARM_LOCK_FILE`.

## Part B corrections and their rule attributions

Measured Windows rename truth table (probe, NTFS): **rule 1** — a handle on an object never
blocks renaming that object when the share mode admits delete; **rule 3** — any open handle
to a descendant blocks renaming any ancestor with `ERROR_ACCESS_DENIED (5)`.

- **B1** `transaction_cleanup_never_deletes_swap_after_validation` — asserted a refusal that
  does not happen (rule 1). Platform split deleted; one body now runs everywhere and
  exercises the real handle-bound cleanup defense instead of a false OS guarantee.
- **B2** `transaction_cleanup_preserves_same_path_replacement` — rule 3 via the fixture's
  retained `RegularFileAuthority` on `.wayland-reservation`. Split kept; fixture coupling
  recorded in-source.
- **B3** `failed_transaction_cleanup_remains_retryable` — rule 3 via the `worker-retry`
  descendant. Comment-only; every assertion, gate and polarity byte-identical.
- **B4** `dispatch_smoke.rs` — rule 3 via the `.swarm-worktrees` handle retained inside
  `repo`. Both helpers re-attributed, the "physically impossible" overclaim removed, and a
  new ungated test restores `validate_repo_authority` coverage on both platforms.

## Empirical determinations (settled by direct attempt on SEANDESKTOP)

Both throwaway probes ran on a scratch branch that was deleted; nothing was committed to
production code.

- **B2:** `PROBE-B2-RESULT: RENAME REFUSED Os { code: 5, PermissionDenied }` — prediction
  HELD. Control with only the root's own handle held:
  `PROBE-B2-CONTROL: RENAME OF HELD OBJECT SUCCEEDED` (rule 1 confirmed).
  → Kept the split with the descendant-rule rationale.
- **B4:** `PROBE-B4-RESULT: RENAME OF repo ITSELF REFUSED Os { code: 5, PermissionDenied }`
  — prediction HELD. → Kept the corrected OS-refusal arms; the new out-of-repository test
  stays and passes on Windows, confirming the topology reasoning.

## Measured results (SEANDESKTOP, final SHA `646ecda8`)

`cargo check -p wcore-sandbox -p wcore-swarm` and `cargo check -p wcore-swarm --all-targets`
both compile.

| Suite | Baseline (185d9612) | Final (646ecda8) |
|---|---|---|
| `wcore-swarm` | 84 run / 72 passed / 12 failed / 6 skipped | 87 run / 78 passed / 9 failed / 6 skipped |
| `wcore-agent --test transactional_delegated_mutation_test` | 9 / 1 passed / 8 failed | 9 / 3 passed / 6 failed |
| `wcore-swarm --test dispatch_smoke` | — | 7 run / 3 passed / 4 failed / 3 skipped |

**Occurrences of `Io(Os { code: 87 })` across both suites: 7 → 0.** Every test this plan
targeted now passes: `failed_transaction_cleanup_remains_retryable`,
`transaction_cleanup_never_deletes_swap_after_validation`,
`transaction_cleanup_preserves_same_path_replacement`, plus the three new tests
(`transaction_lease_is_mutually_exclusive`, `swarm_lock_is_mutually_exclusive`,
`repository_replaced_at_same_pathname_is_refused_by_retained_authority`).

## Residual failures — all pre-existing, none a regression

No test moved pass → fail. The zero-failure target in the plan rested on a predicted
post-20-72 baseline of 84/81/3/6; the re-measured baseline was 84/**72**/**12**/6.

1. **5 aborts, `0xc00000fd` stack overflow** (out of scope, separate diagnosis running):
   `dispatches_4_noop_workers_in_parallel`,
   `swarm_reports_failed_worker_status_and_succeeding_workers_complete`,
   `timeout_releases_workspace_and_capacity_before_return`,
   `multi_worker_output_exhaustion_fails_without_retaining_buffers`,
   `required_live_windows_public_dispatch_bash_confines_parent_and_descendants`.
   The last was previously masked by err-87 and now reaches the same overflow.

2. **4 failures with `Access is denied. (os error 5)` — a distinct, pre-existing
   `wcore-sandbox` Windows cleanup defect, previously masked by err-87.** Root cause
   isolated by bisecting probe:

   ```
   PROBE[empty root]:                    SUCCEEDED
   PROBE[one file child]:                SUCCEEDED
   PROBE[one EMPTY directory child]:     FAILED Io(Os { code: 5, PermissionDenied })
   PROBE[directory child + file]:        FAILED Io(Os { code: 5, PermissionDenied })
   PROBE[two nested directory levels]:   FAILED Io(Os { code: 5, PermissionDenied })
   ```

   `remove_descendants` opens each child with `RelativeKind::Any, RelativeIntent::Mutate`,
   whose access arm grants `FILE_GENERIC_READ | DELETE | SYNCHRONIZE` — **no
   `FILE_GENERIC_WRITE`**. When the child is a directory, that write-less handle becomes a
   `DirectoryAuthority` and the recursion ends in `authority.handle.sync_all()`
   (`FlushFileBuffers`), which requires `GENERIC_WRITE` and returns `ERROR_ACCESS_DENIED`.
   The `(RelativeKind::Directory, Mutate)` arm DOES carry `FILE_GENERIC_WRITE` for exactly
   this reason; the `Any` arm never did. Affected:
   `transaction_workspace_paths_share_manager_representation`,
   `independent_cli_processes_cannot_overbook_shared_capacity`,
   `malformed_heartbeat_fails_closed_and_preserves_bounded_diagnostic`,
   `public_dispatch_owns_git_authority_and_preserves_parent_and_sibling_state`.
   **NOT FIXED HERE:** this plan's scope fence declares "every existing `RelativeIntent`
   arm — untouched".

3. **6 `wcore-agent` failures — the DELETE-bit/chdir defect class that 20-72 owns**,
   surfacing on the integration-checkout authority rather than the repo-root authority:
   `fatal: cannot change to '\\?\C:\...\checkout': Permission denied`. Previously masked by
   err-87. The scope fence declares that defect 20-72's, "entirely", and forbids extending
   the observational open to any further authority. **NOT FIXED HERE.**

## Gate substitutions

- `cargo clippy --workspace --all-targets` is **UNREACHABLE**: `wcore-sandbox`'s test
  targets fail with 19 pre-existing compile errors (`could not compile wcore-sandbox (lib
  test) due to 19 previous errors`), none referencing any symbol this plan adds.
  Substituted `cargo clippy -p wcore-sandbox -p wcore-swarm` (libs) and
  `cargo clippy -p wcore-swarm --all-targets`. Both emit only pre-existing warnings; zero
  come from this plan's code.
- `cargo fmt --all --check` clean on SEANDESKTOP.
- Source gate `cfg(windows)` count in `worktree.rs` equal to the pre-task tree: went 1 → 0,
  not equal. Deleting `create_private_regular_file` (orphaned when both lease sites moved
  to the derivation) removed the file's only `#[cfg(windows)]`. The gate's stated purpose
  — "no platform conditional leaked into `wcore-swarm`" — is satisfied maximally. Leaving
  the function would have produced a `dead_code` warning.

## Recorded unknowns

- All probes ran on NTFS under `%TEMP%` on C:. `FILE_DISPOSITION_INFO_EX` succeeded there,
  so the disposition fallback in `directory_authority_windows.rs` remains UNTESTED on this
  box and may still matter on ReFS, FAT or SMB.
- The `LockFileEx`-on-directory failure is an object-manager property and does NOT depend
  on the filesystem, so the Part A finding generalizes even though the rename-rule probes
  do not necessarily.

No Phase 20 requirement is marked complete — closure is claimed by the downstream native
proof, and the full 6-target re-dispatch waits on the rest of the phase's native repair set
under the existing Sean gate.
