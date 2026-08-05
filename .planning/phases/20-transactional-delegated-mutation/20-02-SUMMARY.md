---
phase: 20-transactional-delegated-mutation
plan: "02"
subsystem: sandbox
tags: [windows, appcontainer, acl, lease, crash-recovery, native-uat]
requires: []
provides:
  - Per-execution AppContainer identities with durable ACL lifecycle leases
  - No-follow identity-checked lease persistence and owner-scoped mutation locking
  - Exact-candidate native Windows ACL acceptance entrypoint
affects: [20-06, windows-sandbox, delegated-mutation]
tech-stack:
  added: []
  patterns: [per-execution authority, durable cleanup state machine, opened-handle identity, exact-candidate UAT]
key-files:
  created:
    - crates/wcore-sandbox/src/backends/appcontainer/acl_lease.rs
    - crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs
    - crates/wcore-sandbox/src/backends/appcontainer/acl_lease/mutation_lock.rs
    - scripts/f20-native-windows-proof.ps1
  modified:
    - crates/wcore-sandbox/src/backends/appcontainer.rs
    - crates/wcore-sandbox/tests/live_fs_acl.rs
    - .github/workflows/nightly-windows-soak.yml
key-decisions:
  - "Persist process-exited, ACL-revoked, profile-deletion-pending, and cleaned boundaries before advancing cleanup authority."
  - "Treat native Windows behavior as later UAT; Windows GNU compilation is not a native PASS."
patterns-established:
  - "One sandbox execution owns one unique AppContainer profile, SID, and durable lease."
  - "Cleanup revokes only the lease-bound SID and retains recoverable evidence until profile deletion completes."
requirements-completed: []
coverage:
  - id: D1
    description: "Unique AppContainer identity and durable ACL lease lifecycle"
    verification:
      - kind: unit
        ref: "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-02-unit ... test -p wcore-sandbox --lib"
        status: pass
      - kind: other
        ref: "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-02-windows-check ... check --target x86_64-pc-windows-gnu -p wcore-sandbox --all-targets --all-features"
        status: pass
      - kind: manual_procedural
        ref: "scripts/f20-native-windows-proof.ps1"
        status: unknown
    human_judgment: true
    rationale: "Native AppContainer behavior requires Sean-authorized exact-candidate Windows UAT."
  - id: D2
    description: "Race-safe crash-recoverable lease persistence and mutation serialization"
    verification:
      - kind: other
        ref: "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-02-clippy ... clippy -p wcore-sandbox --all-targets --all-features -- -D warnings"
        status: pass
      - kind: manual_procedural
        ref: "acl_lease native ignored tests via scripts/f20-native-windows-proof.ps1"
        status: unknown
    human_judgment: true
    rationale: "Crash, reparse, and cross-process behavior still needs native Windows UAT."
  - id: D3
    description: "Commit/tree-pinned native Windows ACL acceptance route"
    verification:
      - kind: other
        ref: "Windows GNU all-targets cross-check at 96afb30aff362ef8f0d4f6f93773eae548d989ee"
        status: pass
      - kind: manual_procedural
        ref: "scripts/f20-native-windows-proof.ps1 -ExpectedCommit <sha> -ExpectedTree <tree>"
        status: unknown
    human_judgment: true
    rationale: "The route is constructed and cross-compiled but intentionally not dispatched by this plan."
duration: 18min
completed: 2026-07-19
status: complete
---

# Phase 20 Plan 02: Transactional Delegated Mutation Summary

**Per-execution Windows AppContainer ACL authority is isolated, durably recoverable, and pinned to an exact-candidate native acceptance route.**

## Performance

- **Duration:** 18 min
- **Started:** 2026-07-19T13:37:09Z
- **Completed:** 2026-07-19T13:54:48Z
- **Tasks:** 3
- **Files modified:** 9

## Accomplishments

- Replaced shared AppContainer ACL authority with unique per-execution profiles, SIDs, intents, and integrity-bound leases.
- Added explicit durable cleanup boundaries, exact-SID revocation, dead-owner recovery, no-follow storage, atomic rewrites, and a global user-keyed mutation mutex.
- Added concurrent/hostile Windows ACL tests plus a helper and nightly route that reject dirty, wrong-repository, wrong-commit, or wrong-tree candidates.

## Task Commits

1. **Task 1: Own a unique AppContainer identity and ACL lease per execution** - `4dcd62a`
2. **Task 2: Make lease storage race-safe and crash-recoverable** - `2ebf46a`
3. **Task 3: Pin native Windows acceptance to the exact candidate** - `ba4f1a2`
4. **Final self-review: Retain in-memory authority until the durable exit transition succeeds** - `96afb30`

## Verification

- Hetzner unit gate: PASS, 43 passed, 0 failed at `96afb30aff362ef8f0d4f6f93773eae548d989ee`.
- Hetzner Windows GNU all-targets/all-features check: PASS at the same exact commit.
- Hetzner clippy all-targets/all-features with `-D warnings`: PASS at the same exact commit.
- Native Windows AppContainer UAT: NOT RUN and NOT CLAIMED; the later authorized entrypoint is `scripts/f20-native-windows-proof.ps1`.

## Salvage Disposition

- `999303eb116640216a4b0b6ad9e50e75461f9cb2`: adopted path-by-path as the cumulative implementation source, then adapted with the explicit lifecycle states, hostile path cases, and exact-candidate proof route required by this plan.
- `b4c3f5aa7d2767c0d01bb41830d122c136606c66`: rejected wherever paths overlap because `999303eb` supersedes it; retained only as provenance.

## Deviations from Plan

The repository-wide `cargo fmt --all -- --check` remains red on two pre-existing files outside this plan's ownership (`crates/wcore-agent/tests/session_journal_test.rs` and `crates/wcore-types/src/child_transaction/tests.rs`). All nine owned Rust paths pass scoped rustfmt, and all owned changes pass diff hygiene. No unrelated formatting was applied.

## Next Phase Readiness

The construction and exact committed-HEAD Hetzner gates are complete. Native Windows behavior remains an explicit later UAT boundary and must not be represented as passed until the pinned helper runs on an authorized Windows candidate.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-19*
