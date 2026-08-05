---
phase: 20-transactional-delegated-mutation
plan: "07"
subsystem: [swarm, agent]
tags: [delegated-mutation, parent-landing, compare-and-swap, quarantined-import, recoverable-rollback, forgery-resistance]
requires: ["20-01", "20-03", "20-05", "20-14"]
provides:
  - A pure parent-owned quarantined-import + git update-ref CAS + coherent projection + recoverable rollback primitive (wcore-swarm, no wcore-agent dep, no SessionJournal)
  - Durable-transaction authorization of prepare/land/rollback gated on the 06C accepted-candidate prerequisite, with 8 versioned landing lifecycle events + deterministic reducer recovery matrix
  - Public-append forgery resistance for all 8 landing authority events (mintable only via the authorized ChildTransactionStore path)
affects: [20-08]
scope_expansion:
  from: 8
  to: 9
  added: [crates/wcore-agent/src/session_journal.rs]
  rationale: "The new landing authority SessionEvents are the parent-MUTATION authority events — strictly more privileged than the 06C ChildTransactionOpened/ReceiptCommitted events already blocked from public SessionJournal::append. The 20-14 audit certified that public-append denylist as part of the forgery-resistance model; leaving landing events publicly appendable is a real security-consistency gap. session_journal.rs (the denylist site) is not among the 8 declared files, so the orchestrator authorized it as a 9th scoped file for the minimal denylist extension. Not a compile blocker — a security-correctness requirement matching a certified pattern."
key-files:
  created:
    - crates/wcore-swarm/src/worktree/parent.rs
    - crates/wcore-agent/src/child_transaction/parent.rs
    - crates/wcore-agent/tests/child_transaction_parent_cas_test.rs
  modified:
    - crates/wcore-swarm/src/worktree.rs
    - crates/wcore-swarm/src/worktree_security.rs
    - crates/wcore-agent/src/child_transaction.rs
    - crates/wcore-agent/src/session_journal.rs
    - crates/wcore-agent/src/session_journal/model.rs
    - crates/wcore-agent/src/session_journal/reducer.rs
key-decisions:
  - "worktree/parent.rs (wcore-swarm) is a PURE parent-owned import/landing primitive with NO wcore-agent dependency and NO SessionJournal access. Pipeline: bind a Wayland-owned integration checkout (refuse arbitrary/linked/bare/dirty targets, alternate stores, detached/ambiguous HEAD, target branch held by an unowned worktree) → hold an exclusive per-ref fd_lock → bind a durable ParentPreimage requiring the ref to name exactly expected_commit → SYNTHESIZE the successor entirely parent-side in a quarantine object dir (read-tree base → add --all --force sealed candidate working tree → write-tree → commit-tree -p base) via scrubbed argv-mode git so the child's .git/hooks/config never load and no child process runs → revalidate type/tree/connectivity/fsck/is-ancestor FROM quarantined bytes → byte-copy promote objects → bind refs/wayland/landing/<slug> → git update-ref <ref> <new> <old> CAS → coherently project + verify HEAD/index/worktree. Returns typed Prepared/RefAdvanced/Projected/Completed/Conflict/RecoveryRequired outcomes carrying exact ParentPreimage/ParentSuccessor identity + a rollback handle."
  - "CORRECTNESS PIVOT: the 06A CandidateSeal binds HEAD==base (gates run against the working tree, no commit is made), so the candidate is base-commit + sealed working-tree diff — landing SYNTHESIZES the tree+commit parent-side rather than importing a pre-existing candidate commit closure."
  - "child_transaction/parent.rs authorizes prepare/land/rollback from durable transaction state gated on the 06C accepted-candidate prerequisite (accepted.transaction_id()==authority.transaction_id() AND transaction.latest_receipt_digest()==accepted.accepted_receipt_digest(); the reducer independently re-enforces the same binding on LandingPrepared). It journals LandingPrepared BEFORE the CAS, maps each typed outcome to RefAdvanced→Projected→Landed / Conflict / RecoveryRequired, and rollback to RollbackPrepared→RolledBack/RecoveryRequired via append_conditionally."
  - "Fail-closed before parent mutation on: parent drift (ref/index/worktree-status/symbolic-HEAD re-checked UNDER THE LOCK immediately before promotion), dirty parent, non-descendant/stale base (candidate.base_commit != expected_commit → Conflict), foreign/missing/corrupt/substituted objects (in-quarantine cat-file/rev-parse/rev-list/fsck), lock contention (try_write), textual conflict (read-tree -m -u). A non-zero update-ref (old-mismatch) → Conflict, never overwriting a drifted ref. Objects promote and are pinned by the quarantine ref BEFORE the CAS; interruption never deletes shared objects (GC owns reclamation). Rollback holds the same lock and reverse-CAS new→old proceeds ONLY while ref+index+clean-worktree still equal the landed successor, else RollbackForeignDrift→RecoveryRequired."
  - "All 8 landing authority events (ChildTransactionLandingPrepared/RefAdvanced/Projected/Landed/Conflict/RecoveryRequired/RollbackPrepared/RolledBack) are rejected by the public SessionJournal::append and mintable ONLY via the pub(crate) append_conditionally ChildTransactionStore path — matching the 20-14-certified 06C forgery-resistance model."
patterns-established:
  - "Child mutation affects the parent only via parent-owned quarantined import, revalidation from quarantined bytes, exact-old CAS under an exclusive lock, coherent projection, and reverse-CAS rollback that never overwrites foreign state — gated on an accepted candidate and recorded as unforgeable durable authority events."
requirements-completed: []
duration: n/a
completed: 2026-07-21
status: complete
source_sha: d527ca87638ac7f6e735fd77bec2616b8fd23478
source_tree: 2d8231cd8547064cdf6e32c70d7cc9e1235e8dac
task_base: 5a30ea88036630e18e841380db9cd29ceb107360
independent_security_review: "Fresh non-author adversarial review prosecuted the landing/CAS/rollback/append-denylist across 9 invariants. Confirmed clean: CAS-under-lock/no-TOCTOU, accepted-candidate prerequisite, rollback-never-overwrites-foreign, append-denylist completeness, crate boundary, deterministic recovery matrix, test non-vacuity. Found 1 MEDIUM (candidate .git/config filter TOCTOU) + 1 LOW (denylist test covered only 3/8 variants) — both fixed and re-proven at source d527ca8."
changed-paths:
  - crates/wcore-swarm/src/worktree.rs
  - crates/wcore-swarm/src/worktree_security.rs
  - crates/wcore-swarm/src/worktree/parent.rs
  - crates/wcore-agent/src/child_transaction.rs
  - crates/wcore-agent/src/child_transaction/parent.rs
  - crates/wcore-agent/src/session_journal.rs
  - crates/wcore-agent/src/session_journal/model.rs
  - crates/wcore-agent/src/session_journal/reducer.rs
  - crates/wcore-agent/tests/child_transaction_parent_cas_test.rs
---

# Phase 20 Plan 07: Parent Landing via Quarantined Import + git update-ref CAS + Recoverable Rollback

**An accepted standalone child candidate integrates into the parent ONLY through parent-owned quarantine, revalidation from quarantined bytes, exact-old `git update-ref` CAS under an exclusive lock, coherent projection, and reverse-CAS rollback that never overwrites foreign state. Linux-proven + adversarially reviewed at source `d527ca8`. Admits 20-08.**

## Admission

- 20-14 pair re-proven green. Task base `gsd-task-base-20-07` = `5a30ea8` / tree `30ed3aa1`.

## Verification (Linux, committed-HEAD Hetzner harness, source `d527ca8`)

- **Scope:** `scope-ok base=5a30ea8 paths=9` (8 declared + the authorized `session_journal.rs` denylist file); `fmt --all -- --check` clean.
- **`clippy -p wcore-swarm -p wcore-agent --all-targets --all-features -- -D warnings`:** clean.
- **`test -p wcore-agent --test child_transaction_parent_cas_test`:** 5 passed / 0 failed — `lands_and_journals_full_lifecycle_then_rolls_back`, `restart_recovery_replays_landed_state_from_disk`, `rollback_refuses_after_foreign_change`, `concurrent_second_lander_cannot_double_land`, `public_append_rejects_landing_authority_events` (all 8 variants).
- **`test -p wcore-swarm --lib`:** 99 passed / 0 failed — incl. the live real-git `parent::tests::live::lands_candidate_working_tree_and_rolls_back`, `stale_base_is_a_conflict`, and the new TOCTOU regression `post_seal_filter_config_fails_closed_and_never_runs`.
- **`test -p wcore-agent --lib`:** unaffected by this localized import-path change; the only deterministic failures remain the 2 pre-existing out-of-scope `spawn_tool` tests (tracked for the 20-08 aggregate).

## Independent adversarial security review (added — most security-critical plan, no formal gate)

A fresh non-author reviewer prosecuted the parent-mutation code across 9 invariants and **confirmed clean**: CAS-under-lock with no TOCTOU, the accepted-candidate prerequisite (fail-closed), quarantine/child isolation, revalidation-from-quarantined-bytes, rollback-never-overwrites-foreign, deterministic recovery matrix, complete append-denylist, clean crate boundary, and non-vacuous tests. It found **one MEDIUM** — a `.git/config` filter TOCTOU: `git add --all` left `GIT_DIR` to discovery and re-loaded the candidate's local config after the seal's last scan, so a `filter.*` clean/smudge planted in that window could execute under the parent's authority. **Closed with defense-in-depth:** (1) `seal.revalidate()` re-scans `.git/config` immediately before `git add` (window → zero), and (2) `GIT_DIR` is now pinned to the trusted parent common git dir so git never loads the candidate config at all. Regression test `post_seal_filter_config_fails_closed_and_never_runs` proves a post-seal malicious filter fails closed, the canary never runs, and the parent ref does not move. The LOW (denylist test covered 3/8 variants) was extended to all 8.

## Explicit non-claims

The swarm primitive makes no gate/receipt/lifecycle claim (that is Task 2's authorization + the 20-08 full lifecycle). No native-runtime, release, or deployment claim. Admits 20-08 (full lifecycle) — which integrates this landing and faces the 20-16 independent review + the aggregate proof.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-21*
