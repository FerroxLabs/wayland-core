---
phase: 20-transactional-delegated-mutation
plan: "03"
subsystem: delegated-mutation
tags: [rust, git, sandbox, worktree, containment]

requires:
  - phase: 20-transactional-delegated-mutation
    provides: canonical RequestedChildWorkspace authority
provides:
  - Standalone child-owned Git checkout and private scratch lifecycle
  - Capacity admission and persisted aggregate workspace reservations
  - Bash sandbox policy bound to delegated checkout and scratch authority
  - Hostile Git-object, metadata, symlink, global-temp, and parent-write tests
affects: [delegated-execution, child-transactions, bash, sandbox, swarm]

tech-stack:
  added: []
  patterns:
    - Parent-pinned non-local shallow clone with child-owned Git authority
    - Canonical delegated roots revalidated immediately before process spawn
    - Persisted per-transaction storage reservation with owner-bound cleanup

key-files:
  created: []
  modified:
    - crates/wcore-swarm/src/worktree.rs
    - crates/wcore-swarm/src/worktree_tests.rs
    - crates/wcore-tools/src/bash.rs
    - crates/wcore-tools/src/workspace_policy.rs
    - crates/wcore-tools/tests/bash_sandbox_routing_test.rs

key-decisions:
  - "Use git clone --no-local --no-hardlinks --depth=1 instead of linked worktrees or shared object storage."
  - "Treat checkout and scratch as two disjoint owner-issued roots; delegated Bash receives no global scratch write grant."
  - "Strip ambient Git authority variables and revalidate canonical roots immediately before both buffered and streaming spawn paths."

patterns-established:
  - "Delegated mutation workspace: exact parent commit, independent Git directory, no remote, no alternates, one reachable commit."
  - "Failure cleanup: an armed owner-bound setup guard removes partial transaction roots on every post-creation error."

requirements-completed: []

coverage:
  - id: D1
    description: "Child mutations use a standalone Git checkout with independent metadata, objects, refs, config, reflogs, hooks, and owner-bound cleanup."
    verification:
      - kind: integration
        ref: "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-03-worktree $(pwd -P) test -p wcore-swarm --lib worktree — 17 passed"
        status: pass
    human_judgment: false
  - id: D2
    description: "Delegated Bash writes are confined to the supplied checkout and private scratch roots while parent, symlink, global-temp, and Git-authority paths fail closed."
    verification:
      - kind: integration
        ref: "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-03-routing $(pwd -P) test -p wcore-tools --test bash_sandbox_routing_test — 17 passed"
        status: pass
    human_judgment: false
  - id: D3
    description: "The exact clean candidate passes strict all-target/all-feature lint for both modified crates."
    verification:
      - kind: other
        ref: "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-03-clippy $(pwd -P) clippy -p wcore-swarm -p wcore-tools --all-targets --all-features -- -D warnings"
        status: pass
    human_judgment: false

duration: 21min
completed: 2026-07-19
status: complete
---

# Phase 20 Plan 03: Transactional Delegated Mutation Summary

**Delegated mutation now runs in a capacity-admitted standalone Git repository whose Bash writes are limited to its canonical checkout and private scratch roots.**

## Performance

- **Duration:** 21 min from first task commit to accepted candidate
- **Started:** 2026-07-19T13:40:55Z
- **Completed:** 2026-07-19T14:02:16Z
- **Tasks:** 2
- **Files modified:** 5 implementation/test files

## Accomplishments

- Materialized exact-commit child repositories without linked-worktree metadata, remotes, alternates, parent history, local hardlinks, or shared object authority.
- Added explicit available-space, safety-margin, per-transaction, and aggregate reservation admission before materialization, with owner-bound cleanup of failed or completed transactions.
- Bound delegated Bash to only the canonical checkout and private scratch roots, redirected temporary/cache environment beneath scratch, stripped Git authority environment, and rejected root substitution before spawn.
- Proved child config/ref/reflog/hook/object corruption cannot alter parent Git bytes or `git fsck`, and proved live Linux sandbox denial for parent, symlink, and host-global-temp writes.

## Task Commits

1. **Task 1: Create and own standalone mutation checkouts** - `0fa8d98063acd8aab3d8bc0391e7804b3c0a4fa8`
2. **Task 2: Bind Bash containment to supplied isolated workspace authority** - `de16c429b2bb6da05a45dd97c11ced704e7a7e04`

Correctness follow-ups:

- `b9da592b36e8434e343873e59811d528d2f127a7` - heap-bound checkout construction future and Clippy repairs
- `4e9539ab4c5d42daae652352943b6f3fd2bca7e4` - heap-allocate the large hostile checkout scenario
- `8dd3ee9842b96346d96bfa879a4c247607eeae79` - run the hostile scenario on an explicitly sized test stack
- `7393d27c041e3276e2197bf17885ecf64a11ddc7` - owner-bound partial cleanup and expanded authority-isolation proof
- `e73ed237d7f148d5b60657551b5676f6145d50b4` - corrupt the child-owned object pack for non-aliased parent proof

## Files Created/Modified

- `crates/wcore-swarm/src/worktree.rs` - standalone transaction checkout, identity, capacity reservation, and cleanup lifecycle.
- `crates/wcore-swarm/src/worktree_tests.rs` - hostile capacity, cleanup, history, metadata, object-corruption, and parent-integrity coverage.
- `crates/wcore-tools/src/bash.rs` - pre-spawn delegated-root validation and ambient Git authority stripping.
- `crates/wcore-tools/src/workspace_policy.rs` - delegated mutation policy with checkout/scratch-only write authority.
- `crates/wcore-tools/tests/bash_sandbox_routing_test.rs` - manifest and live Linux containment proof.

## Decisions Made

- The child receives a standalone depth-one clone made with `--no-local --no-hardlinks`; linked worktrees and parent object sharing remain forbidden.
- Storage capacity is supplied as parent authority, checked before filesystem mutation, and persisted inside the owned transaction root for aggregate admission.
- Bash does not inspect or reinterpret shell text. It consumes the already-selected isolated workspace policy and fails before spawn when either root's identity changes.

## Salvage Disposition

- `ccf824b9de2ff7efd64a2ea7cf720047f9add85d`: adopted and extended the standalone non-local clone pattern in the owned swarm files.
- `e200c0a178feb698af350312a80a33d5b04fc699`: adopted and extended Bash authority-deny and Git-environment stripping in the owned tools files.
- Rejected from both predecessors: any second intent classifier, shell-text downgrade, linked/shared object design, local/hardlink clone, or process-global current-directory assumption.
- Neither predecessor was cherry-picked.

## Deviations from Plan

### Auto-fixed Issues

**1. Strict Clippy exposed needless test borrows**
- **Found during:** Plan-level remote Clippy gate
- **Fix:** Removed the eight needless borrows.
- **Verification:** Final exact-HEAD strict Clippy gate passed.
- **Committed in:** `b9da592b36e8434e343873e59811d528d2f127a7`

**2. The large hostile async scenario exhausted the default test stack**
- **Found during:** Plan-level remote worktree gate
- **Fix:** Heap-bounded the production construction future and ran the large integration scenario on an explicitly sized test thread.
- **Verification:** Final worktree gate passed all 17 tests.
- **Committed in:** `b9da592b36e8434e343873e59811d528d2f127a7`, `4e9539ab4c5d42daae652352943b6f3fd2bca7e4`, `8dd3ee9842b96346d96bfa879a4c247607eeae79`

**3. Post-clone failures could leave a partial transaction root**
- **Found during:** Requirement-by-requirement completion review
- **Fix:** Added an armed owner-bound setup guard and a post-clone failure cleanup test.
- **Verification:** `failed_post_clone_setup_removes_only_the_owned_partial_transaction` passed remotely.
- **Committed in:** `7393d27c041e3276e2197bf17885ecf64a11ddc7`

**4. Initial corruption proof could fall back from a bad loose object to the valid pack**
- **Found during:** Remote hostile worktree gate
- **Fix:** Corrupt the child-owned pack itself and prove the parent object bytes and `git fsck` remain unchanged.
- **Verification:** Final worktree gate passed all 17 tests.
- **Committed in:** `e73ed237d7f148d5b60657551b5676f6145d50b4`

---

**Total deviations:** 4 auto-fixed correctness/test issues.
**Impact on plan:** All changes stayed inside declared files and strengthened required proof; no feature scope was added.

## Issues Encountered

- Workspace-wide `cargo fmt --all -- --check` on the Mac remained blocked by inherited unrelated formatting drift in `crates/wcore-agent/tests/session_journal_test.rs` and `crates/wcore-types/src/child_transaction/tests.rs`. Every owned Rust file was formatted directly with the pinned Rust formatter, `git diff --check` passed, and no unrelated file was modified.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- The standalone checkout and Bash containment substrate is ready for exact serial integration.
- No plan-level blocker remains. Main merge, push, release, deployment, and issue closure were not performed.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-19*
