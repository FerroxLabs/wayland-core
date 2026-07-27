---
status: investigating
trigger: "Windows-only: (G2) 2 wcore-swarm tests fail Io(Os{code:87,InvalidInput}) from cleanup.release(); (G3) transaction_cleanup_never_deletes_swap_after_validation expect_err fires because a rename of a handle-held dir SUCCEEDED. Diagnose only."
created: 2026-07-25T00:00:00Z
updated: 2026-07-25T00:00:00Z
---

## Current Focus

hypothesis: |
  G2: release() -> with_directory_lock (worktree.rs:329-346) calls fd_lock::RwLock::write()
      on a DIRECTORY handle. fd-lock 4.0.4 windows/rw_lock.rs write() -> LockFileEx().
      LockFileEx is unsupported on directory handles -> ERROR_INVALID_PARAMETER (87).
      SwarmError::Io(#[from] io::Error) prints exactly Io(Os{code:87,kind:InvalidInput}).
  G3: FILE_SHARE_DELETE PERMITS rename of the held object. The test premise is inverted.
      The sibling test's expect_err only passes for an unrelated reason (an open CHILD
      file handle blocks a parent-directory rename).
test: standalone raw-Win32 probe on seandesktop; plus live run of the 3 tests
expecting: LockFileEx on dir -> 87; LockFileEx on file -> success; rename truth table
next_action: run the 3 named tests on seandesktop and capture verbatim output

## Symptoms

expected: 3 wcore-swarm tests pass on Windows
actual: |
  worktree::tests::failed_transaction_cleanup_remains_retryable (worktree_tests.rs:110)
    -> Io(Os { code: 87, kind: InvalidInput }) from cleanup.release()
  worktree::tests::transaction_cleanup_preserves_same_path_replacement (worktree_tests.rs:218)
    -> Io(Os { code: 87, kind: InvalidInput }) from cleanup.release()
  worktree::tests::transaction_cleanup_never_deletes_swap_after_validation (worktree_tests.rs:276)
    -> panics ": ()" i.e. expect_err on std::fs::rename saw Ok
errors: "Io(Os { code: 87, kind: InvalidInput, message: \"The parameter is incorrect.\" })"
reproduction: ssh SeanD@seandesktop; C:\ferrox-win @ ce148c3a; cargo nextest run -p wcore-swarm --lib
started: pre-existing; identical at base 334f264d

## Eliminated

## Evidence

- timestamp: 2026-07-25
  checked: fd-lock 4.0.4 src/sys/windows/rw_lock.rs (registry source, Mac)
  found: RwLock::write() = LockFileEx(handle, LOCKFILE_EXCLUSIVE_LOCK, 0, 1, 0, overlapped)
  implication: any fd_lock use on a directory handle becomes a LockFileEx call on a directory

- timestamp: 2026-07-25
  checked: crates/wcore-swarm/src/error.rs
  found: SwarmError::Io(#[from] std::io::Error) -- the only variant that Debug-prints as Io(Os{..})
  implication: the 87 is a raw io::Error surfacing through a `?`, not a formatted string

- timestamp: 2026-07-25
  checked: crates/wcore-sandbox/src/directory_authority_windows.rs:49-77
  found: open_directory = GENERIC_READ|GENERIC_WRITE|DELETE + FILE_SHARE_READ|WRITE|DELETE
         + FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT;
         open_regular_file = GENERIC_READ + FILE_SHARE_READ|WRITE|DELETE
  implication: test fixture holds a dir handle on root AND a file handle on root/.wayland-reservation

- timestamp: 2026-07-25
  checked: raw Win32 probe (PART A) on seandesktop, Windows 11 10.0.26200.8875, NTFS
  found: |
    LockFileEx(dir handle, LOCKFILE_EXCLUSIVE_LOCK,0,1,0,ov) -- the EXACT fd-lock 4.0.4 call:
      DIR access=GR|GW|DELETE share=R|W|D  -> FAIL GetLastError=87
      DIR access=GR|GW        share=R|W|D  -> FAIL GetLastError=87
      DIR access=GR           share=R|W|D  -> FAIL GetLastError=87
      FILE access=GR|GW       share=R|W|D  -> OK
    Elimination probes on the same dir handle:
      FlushFileBuffers (File::sync_all)                 -> OK
      SetFileInformationByHandle(FileDispositionInfoEx) -> OK   (POSIX delete IS supported here)
      SetFileInformationByHandle(FileDispositionInfo)   -> OK
  implication: |
    87 comes from LockFileEx on a DIRECTORY handle, independent of desired access.
    The FILE_DISPOSITION_INFO_EX hypothesis is REFUTED -- both disposition classes work.

- timestamp: 2026-07-25
  checked: crates/wcore-swarm/src/worktree_security.rs (the wcore-swarm DirectoryAuthority newtype)
  found: |
    open            -> SwarmError::DispatchAdmission(String)   (:35)
    validate_path   -> SwarmError::DispatchAdmission(String)   (:41)
    remove_open_dir_all -> SwarmError::WorktreeIo(String)      (:62)
    try_clone_handle -> .map_err(Into::into) -> SwarmError::Io (:45)
  implication: |
    Every SandboxError is STRINGIFIED. The only SwarmError::Io producers reachable from
    TransactionCleanup::release() are try_clone_handle and with_directory_lock's lock.write()
    -- both inside with_directory_lock (worktree.rs:334-343). The observed panic is the `Io`
    variant, not `WorktreeIo`, which excludes remove_open_dir_all entirely.

- timestamp: 2026-07-25
  checked: raw Win32 probe (PART B) rename truth table, 13 cases
  found: |
    handle on the OBJECT ITSELF, share includes FILE_SHARE_DELETE -> rename SUCCEEDS
      (B2 GR|GW|DELETE, B4 GR-only, B12 empty dir, B13) -- access mode irrelevant
    handle on the OBJECT ITSELF, share WITHOUT FILE_SHARE_DELETE  -> Err 32 SHARING_VIOLATION (B3)
    ANY open handle on a DESCENDANT (file or dir, any share mode) -> ancestor/parent rename
      Err 5 ACCESS_DENIED -> PermissionDenied (B5,B6,B7,B8,B9,B10,B11)
  implication: |
    FILE_SHARE_DELETE PERMITS renaming the held object -- the test comments have it backwards.
    The two passing expect_err assertions pass by the DESCENDANT rule, not the share-mode rule.

- timestamp: 2026-07-25
  checked: raw Win32 probe C -- handle-bound delete after a same-path swap
  found: |
    open dir handle on root; rename root -> moved (SUCCEEDS);
    create fresh root + replacement-receipt;
    GetFinalPathNameByHandleW(handle) now reports `moved` (handle follows the OBJECT);
    SetFileInformationByHandle(FileDispositionInfoEx) -> OK;
    RESULT: substituted dir survives, its receipt reads back, the moved ORIGINAL is gone.
  implication: |
    The production mechanism (handle-bound delete) satisfies the exact Unix assertions on
    Windows. No production hole -- the swap is constructible, and the defense still works.

- timestamp: 2026-07-25
  checked: live run of dispatch_smoke repository_replacement tests on seandesktop
  found: dispatch_rejects_same_head_repository_replacement PASS;
         dispatch_rejects_different_head_repository_replacement PASS
  implication: 334f264d's ANCESTOR-rename assumption is sound and non-vacuous.

## Resolution

root_cause: |
  GROUP 2 (error 87): crates/wcore-swarm/src/worktree.rs:336-343. with_directory_lock wraps a
  DIRECTORY handle (DirectoryHandleLoan from DirectoryAuthority::try_clone_handle) in
  fd_lock::RwLock and calls .write(). fd-lock 4.0.4 src/sys/windows/rw_lock.rs::write issues
  LockFileEx(handle, LOCKFILE_EXCLUSIVE_LOCK, 0, 1, 0, &overlapped). Windows byte-range locking
  is not defined on directory objects: LockFileEx returns FALSE with ERROR_INVALID_PARAMETER (87)
  for ANY directory handle regardless of desired access. worktree.rs:341 converts it via
  SwarmError::Io (error.rs:41 #[from] std::io::Error), printing
  Io(Os { code: 87, kind: InvalidInput, message: "The parameter is incorrect." }).
  Two more directory-lock sites are broken the same way: ActiveLease::acquire (worktree.rs:122)
  and transaction_is_active (worktree.rs:453).

  GROUP 3 (inverted premise): FILE_SHARE_DELETE is what PERMITS renaming an open object, not
  what forbids it. The three tests' Windows comments assert the opposite. Only the
  descendant-handle rule (any open handle below a directory blocks renaming that directory)
  actually refuses anything. The TEST is wrong; the production invariant is intact, because
  remove_open_dir_all deletes by retained handle and the handle follows the renamed object.
fix: NOT APPLIED (diagnose-only mode)
verification: n/a
files_changed: []
