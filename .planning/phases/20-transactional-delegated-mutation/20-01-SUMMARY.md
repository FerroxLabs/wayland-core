---
phase: 20-transactional-delegated-mutation
plan: "01"
subsystem: agent-persistence
tags: [rust, session-journal, delegated-mutation, replay, authority]

requires:
  - phase: 20-transactional-delegated-mutation
    provides: "Accepted wcore-types child transaction receipt contract at b9cc6698f2b43a04f1b4deee7064def8f754d9e7"
provides:
  - "Opaque journal-derived transaction opening authority"
  - "Durable child transaction opening and receipt projection"
  - "Hostile replay, substitution, corruption, and legacy coverage"
affects: [20-transactional-delegated-mutation, child-launch, merge, rollback, cleanup]

tech-stack:
  added: []
  patterns:
    - "Authority is minted only under the live journal writer lease after durable opening"
    - "Transaction evidence is validated before append and reduced deterministically on replay"

key-files:
  created:
    - crates/wcore-agent/src/child_transaction.rs
    - crates/wcore-agent/tests/child_transaction_store_test.rs
  modified:
    - crates/wcore-agent/src/lib.rs
    - crates/wcore-agent/src/session_journal.rs
    - crates/wcore-agent/src/session_journal/model.rs
    - crates/wcore-agent/src/session_journal/reducer.rs
    - crates/wcore-agent/src/session_journal/snapshot.rs

key-decisions:
  - "Derive the opaque token digest from the committed opening envelope sequence and checksum, avoiding caller-minted or circular authority."
  - "Bind opening authority to the normalized journal storage identity so copied journal bytes cannot rebind an existing token."
  - "Reject authority-only transaction events through the public SessionJournal append surface."

patterns-established:
  - "Commit under retained authority: verify the snapshot sidecar, append under one writer lock, then return opaque authority."
  - "Replay-first inspection: durable journal state, not caller or child claims, is the transaction source of truth."

requirements-completed: []

coverage:
  - id: D1
    description: "Transactions open from opaque retained journal authority before external effects."
    verification:
      - kind: integration
        ref: "cargo test -p wcore-agent --test child_transaction_store_test"
        status: pass
      - kind: other
        ref: "cargo clippy -p wcore-agent -p wcore-types --all-targets --all-features -- -D warnings"
        status: pass
    human_judgment: false
  - id: D2
    description: "Validated child transaction receipts persist and replay deterministically with fail-closed conflict handling."
    verification:
      - kind: integration
        ref: "cargo test -p wcore-agent --test child_transaction_store_test"
        status: pass
    human_judgment: false
  - id: D3
    description: "Substitution, corruption, reordering, public authority minting, and legacy replay are covered by hostile tests."
    verification:
      - kind: integration
        ref: "crates/wcore-agent/tests/child_transaction_store_test.rs (8 tests on Hetzner)"
        status: pass
    human_judgment: false

duration: 31min
completed: 2026-07-19
status: complete
---

# Phase 20 Plan 01: Transactional Delegated Mutation Summary

**Journal-authoritative child transaction openings and receipts now persist, replay, and fail closed under hostile evidence.**

## Performance

- **Duration:** 31 min
- **Started:** 2026-07-19T13:38:40Z
- **Completed:** 2026-07-19T14:09:32Z
- **Tasks:** 3
- **Files modified:** 7

## Accomplishments

- Added `ChildTransactionStore` and `ChildTransactionWrite`, with opaque authority returned only after a durable opening append under the live writer lease.
- Added deterministic journal projection for openings and committed receipts, including exact-retry idempotency and fail-closed conflict/order validation.
- Added eight hostile integration tests covering retained authority, zero-effect failures, public minting rejection, copied/symlinked/multi-linked storage, corrupt/reordered/truncated persistence, receipt retry, and legacy replay.

## Task Commits

1. **Task 1: Open transactions from retained snapshot authority and implement the store**
   - `df987391306d61e48c2d1dea344e756310afa802` (`feat`)
   - `9f6577e868098552c982e0d33f407dd1a2974316` (`fix`)
   - `c238218313a0e62cfb73cab6b1070970fbd46741` (`fix`)
2. **Task 2: Reduce committed receipts into deterministic session state**
   - `feb8f5ccecdb0485a1d2c647955814ea1acd45d2` (`feat`)
   - `dfe7fe33b1777f70669a8cbd522d94357ac18dae` (`fix`)
3. **Task 3: Prove hostile replay and backward compatibility**
   - `3b67dbebb77cf542221ff288d55002b79a526c03` (`test`)
4. **Strict acceptance cleanup**
   - `84568f81f30585e0824fd91435b20aaadc662b47` (`chore`)
   - `32773e9c593ffa403d0197ea60a52639c7a6a743` (`chore`)
   - `626e1d4d3dee9fee7008ad172ec0b4add8f2004e` (`chore`)

## Files Created/Modified

- `crates/wcore-agent/src/child_transaction.rs` - Opaque opening authority plus transaction store, revalidation, commit, inspect, and list APIs.
- `crates/wcore-agent/src/lib.rs` - Exposes the transaction module.
- `crates/wcore-agent/src/session_journal.rs` - Writer-lease acquisition, snapshot-sidecar verification, storage binding, and authority-only public-append rejection.
- `crates/wcore-agent/src/session_journal/model.rs` - Durable opening/receipt events and reduced transaction state.
- `crates/wcore-agent/src/session_journal/reducer.rs` - Deterministic opening and receipt validation/reduction.
- `crates/wcore-agent/src/session_journal/snapshot.rs` - Test-scope strict-lint correction in the authority path.
- `crates/wcore-agent/tests/child_transaction_store_test.rs` - Hostile persistence and backward-compatibility tests.

## Salvage Disposition

Commit `d6805a5897a3e5f9d148eda489a09a38ae803a1a` was reviewed path by path and was not cherry-picked:

- `crates/wcore-agent/src/child_transaction.rs` - **adapted** to retained snapshot authority, durable opening, storage binding, and current accepted types.
- `crates/wcore-agent/src/lib.rs` - **adapted** by exposing the new module.
- `crates/wcore-agent/src/session_journal/model.rs` - **adapted** to persist both authoritative opening and receipt state.
- `crates/wcore-agent/src/session_journal/reducer.rs` - **adapted** with full lifecycle, conflict, ordering, and token validation.
- `crates/wcore-agent/tests/child_transaction_store_test.rs` - **adapted and expanded** for current snapshot authority and hostile persistence requirements.
- `crates/wcore-types/src/child_transaction.rs` - **rejected**; accepted baseline types remain authoritative.
- `crates/wcore-types/src/child_transaction/tests.rs` - **rejected** with the rejected type change.

## Decisions Made

- Kept the accepted `wcore-types` contract unchanged and constructed all new binding evidence in `wcore-agent`.
- Derived token identity from the committed opening envelope, rather than trusting caller serialization.
- Allowed exact receipt retries to remain idempotent even after the child revision advances, while successor receipts still require the prior committed receipt.
- Bound authority to storage identity in addition to journal content so byte-for-byte copied journals cannot reuse the original opaque authority.

## Verification

Exact source HEAD: `626e1d4d3dee9fee7008ad172ec0b4add8f2004e`

- PASS: `/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-01-test "$(pwd -P)" test -p wcore-agent --test child_transaction_store_test`
  - Hetzner result: 8 passed, 0 failed, 0 ignored.
- PASS: `/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-01-clippy "$(pwd -P)" clippy -p wcore-agent -p wcore-types --all-targets --all-features -- -D warnings`
  - Hetzner result: exit 0; strict Clippy passed.
- PASS: `git diff --check e6706393141f3fbcaacfef1d42617828cd5b19ed..HEAD`.
- PASS: changed-path ownership; all seven changed source/test paths are declared by this plan.
- No native Windows claim is made.

## Deviations from Plan

### Auto-fixed Issues

**1. Authority-only events were publicly appendable**
- **Found during:** Task 3 hostile testing.
- **Issue:** A caller could append a transaction opening event through the generic public journal API.
- **Fix:** Rejected transaction authority events on the public append path while retaining internal authoritative append/replay.
- **Verification:** Hostile integration test passes on Hetzner.
- **Committed in:** `9f6577e868098552c982e0d33f407dd1a2974316`.

**2. Copied journal bytes could reuse storage-neutral authority**
- **Found during:** Task 3 hostile testing.
- **Issue:** Content-identical journal bytes copied to another store needed a distinct authority identity.
- **Fix:** Bound authority to the normalized journal storage identity and revalidated it before use.
- **Verification:** Copied-journal rebind test passes on Hetzner.
- **Committed in:** `9f6577e868098552c982e0d33f407dd1a2974316` and `c238218313a0e62cfb73cab6b1070970fbd46741`.

**3. Strict Clippy exposed plan-scope test and projection lints**
- **Found during:** Plan-level acceptance gate.
- **Issue:** The exact all-targets/all-features `-D warnings` gate exposed enum-size and test-hook lint findings.
- **Fix:** Boxed the large internal projection variant, simplified plan-scope control flow, and named the test hook type.
- **Verification:** Exact strict Clippy command passes on Hetzner.
- **Committed in:** `dfe7fe33b1777f70669a8cbd522d94357ac18dae`, `84568f81f30585e0824fd91435b20aaadc662b47`, `32773e9c593ffa403d0197ea60a52639c7a6a743`, and `626e1d4d3dee9fee7008ad172ec0b4add8f2004e`.

---

**Total deviations:** 3 auto-fixed correctness/acceptance issues.
**Impact on plan:** All changes remain inside the seven declared paths and strengthen the stated authority and replay contract.

## Issues Encountered

`cargo fmt --all -- --check` reports two pre-existing baseline formatting diffs in undeclared paths: `crates/wcore-agent/tests/session_journal_test.rs` and `crates/wcore-types/src/child_transaction/tests.rs`. This plan did not modify either file. The formatter reported no diff in any of the seven plan-owned paths; source diff hygiene and changed-path ownership pass.

## User Setup Required

None.

## Next Phase Readiness

The authoritative transaction store is ready for dependent F20 workspace, execution, merge, rollback, and cleanup plans. Integration must preserve the exact accepted source commit and rerun dependent aggregate gates after landing.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-19*
