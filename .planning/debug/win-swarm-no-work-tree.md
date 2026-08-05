---
status: diagnosed
trigger: "Windows-only: 13 wcore-swarm tests fail with WorktreeIo(\"git status failed: fatal: this operation must be run in a work tree\") at ce148c3a. Determine if this is a NEW regression from ece09681..919629ee (normalized_root centralization) or a PRE-EXISTING defect previously MASKED by the BL-0 'refused changed worktree root' abort."
created: 2026-07-25T00:00:00Z
updated: 2026-07-25T00:00:00Z
---

## Current Focus

hypothesis: CONFIRMED — an open Windows directory handle carrying DELETE access on
  `repo_root` (`DirectoryAuthority::open`, wcore-sandbox/src/directory_authority_windows.rs:57)
  makes SetCurrentDirectory/chdir INTO that directory fail with a sharing violation.
  Git commands flagged NEED_WORK_TREE call setup_work_tree() -> chdir_notify() -> chdir(),
  which fails, and git dies "fatal: this operation must be run in a work tree".
test: standalone Win32 probe isolating the DELETE access bit; GIT_TRACE_SETUP on the live test
expecting: status fails only while a DELETE-access handle is held; rev-parse/config unaffected
next_action: NONE — diagnosis complete, no fix applied per instructions

## Symptoms

expected: wcore-swarm tests pass on Windows
actual: 13 tests fail; 10 with the work-tree error, 2 with ERROR_INVALID_PARAMETER(87), 1 with a false rename-refusal assertion
errors: |
  fatal: this operation must be run in a work tree
  emitted at crates/wcore-swarm/src/worktree_manager.rs:578 (assert_clean, NOT worktree_cleanup.rs:578)
reproduction: ssh SeanD@seandesktop; C:\ferrox-win @ ce148c3a; cargo nextest run -p wcore-swarm --no-fail-fast
started: pre-existing; unmasked by ece09681..919629ee removing the BL-0 abort

## Eliminated

- hypothesis: repo_root points at a bare repo / a .git dir / core.bare=true
  evidence: hand-replicated fixture on the box — rev-parse --is-bare-repository=false,
    --show-toplevel correct, --git-dir=.git, core.bare=false, and `git status` SUCCEEDS
    with the identical env scrub + cwd. Repo layout is not the cause.
  timestamp: 2026-07-25
- hypothesis: normalized_root/dunce leaves a verbatim \\?\ cwd that git cannot consume
  evidence: base error text prints a PLAIN stored root (C:\Users\seand\AppData\Local\Temp\...),
    and GIT_TRACE_SETUP shows git resolving cwd AND worktree to the correct plain path.
    The strip did apply; representation is not the cause.
  timestamp: 2026-07-25
- hypothesis: it is a NEW regression from ece09681..919629ee
  evidence: the identical error string appears at base 334f264d in
    workspace_authority::independent_cli_processes_cannot_overbook_shared_capacity;
    `git diff 334f264d..ce148c3a -- crates/wcore-sandbox/` is EMPTY; the DELETE access bit
    dates to 8738b24e (2026-07-20); DirectoryAuthority::open(&repo_root) exists at base
    (worktree_manager.rs:16 and :120 in `git show 334f264d:`).
  timestamp: 2026-07-25
- hypothesis: all 13 failures share one root cause
  evidence: 3 of 13 have entirely different error text (ERROR_INVALID_PARAMETER 87 at
    worktree_tests.rs:110 and :218; a failed expect_err at :276) and fail IDENTICALLY at base.
  timestamp: 2026-07-25

## Evidence

- timestamp: 2026-07-25
  checked: full `cargo nextest run -p wcore-swarm --no-fail-fast` at ce148c3a on seandesktop
  found: 83 tests run, 70 passed, 13 failed, 6 skipped
  implication: baseline for comparison established

- timestamp: 2026-07-25
  checked: same command at base 334f264d in a separate --shared clone under Temp
  found: 80 tests run, 65 passed, 15 failed, 6 skipped. 9 failures carry the BL-0
    "refused changed worktree root" text; 1 already carries the exact work-tree error;
    3 match the current non-git failures verbatim; 2 are capacity-probe assertion failures.
  implication: 12 of today's 13 failures are in the base-15 set. The 13th
    (transaction_workspace_paths_share_manager_representation) is a NEW test added by 919629ee.
    3 base failures now PASS. Net: 15 -> 12 pre-existing, +1 new test, 0 new breakage.

- timestamp: 2026-07-25
  checked: GIT_TRACE / GIT_TRACE_SETUP on the live failing test
  found: the `git status` process prints its full setup block
    (git_dir=.git, worktree=<fixture>, cwd=<fixture>, prefix=null) and then dies WITHOUT
    emitting `chdir-notify.c:86 setup: chdir from X to X` and WITHOUT `git.c:502 built-in:`.
    Every succeeding command (add/commit) DOES emit the chdir line.
  implication: git found the repo and a valid work tree. It died inside setup_work_tree(),
    at `chdir_notify(work_tree)`, i.e. the chdir itself failed. Not a repo-shape problem.

- timestamp: 2026-07-25
  checked: standalone Rust probe replicating directory_authority_windows.rs:57-61 exactly
  found: |
    [1] no handle                                  -> git status exit 0
    [2] access=GENERIC_READ|GENERIC_WRITE|DELETE   -> git status exit 128
        "fatal: this operation must be run in a work tree"
        (rev-parse exit 0, config exit 0 in the SAME state)
    [3] access=GENERIC_READ|GENERIC_WRITE          -> git status exit 0
    [4] access=GENERIC_READ|DELETE                 -> git status exit 128 (same error)
    [5] all handles dropped                        -> git status exit 0
    [6] cmd `cd /d <repo>` while handle held       -> exit 1 (SetCurrentDirectory refused)
  implication: the DELETE access bit is the sole cause, isolated. Commands that do not
    require a work tree never chdir and are unaffected — which is exactly why pinned_head()
    (rev-parse) and reject_executable_checkout_config() (config) succeed immediately before.

- timestamp: 2026-07-25
  checked: second probe for the post-fix next layer (`git -C <dir>`)
  found: |
    [A] git -C <checkout> status, no handle              -> exit 0
    [B] git -C <checkout> status, DELETE handle on it    -> exit 128
        "fatal: cannot change to '<checkout>': Permission denied"
    [C] same, handle without DELETE                      -> exit 0
    [D] DELETE handle on the PARENT only, cwd=<checkout> -> exit 0
  implication: the block applies to the held directory itself, not its children.
    `run_checkout_git` uses `git -C <checkout>` and will hit the same wall if any
    authority holds the checkout directory with DELETE. Predicted, not yet observed —
    the failing tests abort before a checkout exists.

- timestamp: 2026-07-25
  checked: repo_authority usage across the crate
  found: it is consumed ONLY by validate_repo_authority() ->
    repo_authority.validate_path(&self.repo_root) (worktree_manager.rs:181-183).
    Never mutates, deletes or renames.
  implication: GENERIC_WRITE and DELETE are unnecessary for this particular authority;
    an observational read-only handle satisfies every call site.

## Resolution

root_cause: |
  crates/wcore-sandbox/src/directory_authority_windows.rs:57 opens every DirectoryAuthority with
  `access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)`. crates/wcore-swarm/src/worktree_manager.rs:16
  and :119 use that to hold an authority on `repo_root` for the manager's whole lifetime.
  On Windows, a live handle granted DELETE access on a directory causes SetCurrentDirectory
  (and therefore MSYS/Cygwin chdir) INTO that directory to fail with a sharing violation.
  Git subcommands flagged NEED_WORK_TREE (`status`) call setup_work_tree(), whose
  `chdir_notify(work_tree)` then fails, and git reports
  "fatal: this operation must be run in a work tree".
  assert_clean (worktree_manager.rs:570-586) runs `git status --porcelain` with
  current_dir(repo_root), so it fails 100% of the time on Windows whenever a manager exists.
  Pre-existing since 8738b24e (2026-07-20); untouched by ece09681..919629ee.
fix: NOT APPLIED (diagnose-only mode)
verification: n/a
files_changed: []
