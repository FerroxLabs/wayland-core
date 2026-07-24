---
phase: 20-transactional-delegated-mutation
plan: "51"
subsystem: infra
tags: [native-repair, re-seal, lock-no-op, zero-lock-delta, further-repaired-successor, cargo-locked, aggregate-floor, no-regression, hetzner-gate, honest-deferral, candidate-anchor]

# Dependency graph
requires:
  - phase: "20-50"
    provides: "FURTHER-REPAIRED SUCCESSOR candidate source_sha f0dd5b6d312af616f268f96f34c3bc9fc962c4d2 (tree ac76c87b318ee4ba8c34927dea23e40e63fd0776) over sealed-but-RED 3f839309 — both 20-45-panel findings closed, touching ONLY the two Windows-only files (containment test + cmd classifier), NO manifest change."
provides:
  - "SEALED further-repaired-successor candidate source_sha f0dd5b6d312af616f268f96f34c3bc9fc962c4d2 (tree ac76c87b318ee4ba8c34927dea23e40e63fd0776) — the ONE exact SHA all downstream gates (20-52 four-way pre-native cross-audit, 20-53 native re-dispatch, 20-54 native-inclusive review, 20-55 re-prep, 20-56 terminal) bind to. After this seal, NO source/test/workflow change follows."
  - "ZERO Cargo.lock delta CONFIRMED: unlike the 20-44 seal (which resynced the lock for a new wcore-swarm→dunce direct edge), the 20-50 fix changed no Cargo.toml (two #![cfg(windows)]/windows_impl files only), so the lock is byte-identical to the sealed-but-RED predecessor. git diff 3f839309 f0dd5b6d -- Cargo.lock is EMPTY. A --locked --workspace --all-features Hetzner build against a detached checkout of the exact tree ac76c87b succeeded WITHOUT a stale-lock report and WITHOUT regenerating the lock — no new crate pulled (REQ-native-r10, gate-proven; requirement not claimed)."
  - "LINUX NO-REGRESSION floor green on the sealed successor: Hetzner aggregate nextest --workspace --profile ci --no-fail-fast = 11509 passed / 0 failed / 48 skipped (exit 0). Both 20-50 edits are Windows-only, so Linux compiled output is unchanged — the count is identical to the sealed baseline, an honesty gate not a proof of the Windows fixes (REQ-native-r4, gate-proven; requirement not claimed)."
affects: ["20-52", "20-53", "20-54", "20-55", "20-56"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "No-op-lock re-seal: when the antecedent fix changes no Cargo.toml, confirm zero lock delta by proving a --locked --workspace --all-features build against a detached checkout of the exact source-complete HEAD succeeds without the lock being reported stale or regenerated — the lock counterpart of the 20-44 dunce-edge resync, inverted to a confirmation-of-no-change."
    - "Anchor the sealed candidate by DETACHING HEAD to the source-complete SHA (f0dd5b6d, tree ac76c87b) before invoking the remote-cargo harness, so the harness's HEAD-tree gate verifies the precise sealed candidate tree rather than the doc-tip tree (b8a1c9d4/c42f430a, which differs only by .planning/)."

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-51-SUMMARY.md
  modified: []

key-decisions:
  - "Proved against a DETACHED checkout of the exact 20-50 source-complete HEAD f0dd5b6d (tree ac76c87b), NOT the branch doc-tip b8a1c9d4 (tree c42f430a). The two trees differ ONLY by the 20-50-SUMMARY.md planning doc (git diff --stat f0dd5b6d b8a1c9d4 = 1 file, .planning/ only), which does not affect the build — but the remote-cargo harness hard-verifies the shipped HEAD tree equals the local HEAD tree, so detaching to f0dd5b6d makes the gate bind to the precise sealed candidate tree ac76c87b. After the two gates, HEAD was returned to the branch."
  - "ZERO lock delta was confirmed three ways: (1) git diff 3f839309 f0dd5b6d -- Cargo.lock is EMPTY (the 20-50 fix introduced no lock change); (2) git diff 3f839309 b8a1c9d4 -- Cargo.lock is EMPTY (no lock change through the doc tip either); (3) the --locked --workspace --all-features Hetzner build succeeded (exit 0) WITHOUT the lock being reported stale and WITHOUT regeneration — a stale/incoherent lock would have failed --locked. No new crate appeared, so no package-legitimacy checkpoint was triggered. This is the no-op counterpart of the 20-44 lock resync."
  - "requirements-completed is [] — NO Phase-20 requirement is claimed. REQ-native-r10 (lock consistency) and REQ-native-r4 (Linux no-regression) are IMPLEMENTED as green gates here on Linux, but their FULL completion is bound to the native msvc re-dispatch (20-53) and the terminal seal (20-56); this plan is a re-seal + Linux floor, not a requirement completion. The two 20-50 Windows-only fixes' compile+behavior remain deferred to 20-53."
  - "The metadata commit contains ONLY 20-51-SUMMARY.md (matching the 20-50 doc-commit convention: git show --stat b8a1c9d4 = 1 file). No source, test, workflow, or lock file was modified by this plan; the scope gate confirms the entire change-union from the plan base is exactly this one planning doc."

patterns-established:
  - "A re-seal after a manifest-neutral fix is a CONFIRMATION, not a mutation: run the --locked all-features build to prove the lock is coherent+unchanged, run the aggregate to prove the Linux floor is unregressed, then record the source-complete HEAD verbatim as the sealed candidate. Fabricating a lock commit when there is no delta is forbidden."

requirements-completed: []  # NO Phase-20 requirement claimed. REQ-native-r10/r4 are gate-proven on Linux here; their completion is deferred to the 20-53 native re-dispatch / 20-56 terminal seal.

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "Zero Cargo.lock delta confirmed and the sealed successor builds --locked --workspace --all-features on Hetzner against the exact tree ac76c87b (lock coherent + unchanged, no new crate)."
    requirement: "REQ-native-r10"
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-51-build <detached f0dd5b6d> build --workspace --all-features --locked"
        status: pass
    human_judgment: false
  - id: D2
    description: "Linux no-regression floor: the sealed successor passes the aggregate nextest --profile ci at 11509/0/48 (Windows-only 20-50 edits => unchanged Linux output)."
    requirement: "REQ-native-r4"
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-51-aggregate <detached f0dd5b6d> nextest run --workspace --profile ci --no-fail-fast"
        status: pass
    human_judgment: false

# Metrics
duration: ~12min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 51: Re-seal the further-repaired successor — zero-lock-delta confirmation + Linux no-regression floor Summary

**The further-repaired successor `f0dd5b6d` (tree `ac76c87b`) is SEALED as the one exact candidate every downstream gate (20-52…20-56) binds to. Because the 20-50 fix changed no `Cargo.toml` (two Windows-only files only), this is a no-op-lock re-seal: `Cargo.lock` is byte-identical to the sealed-but-RED predecessor `3f839309`, and a `--locked --workspace --all-features` Hetzner build against a detached checkout of the exact tree `ac76c87b` succeeded (exit 0) WITHOUT the lock being reported stale, WITHOUT regeneration, and with no new crate. The Linux no-regression floor is green: aggregate `nextest --profile ci` = 11509/0/48. No source/test/workflow/lock change follows; native proof + requirement completion stay deferred to 20-53/20-56.**

## Sealed candidate identity

- **source_sha (SEALED):** `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2`
- **source_tree (SEALED):** `ac76c87b318ee4ba8c34927dea23e40e63fd0776` (verified `f0dd5b6d^{tree}` == `ac76c87b`)
- **predecessor (sealed-but-RED, 20-45 BLOCK):** `3f839309574d6741eed416cd3820f56447f74eba` (tree `3092475bb4102d010b6ff5f6c9d8080cb4f51928`)
- **branch:** `plan/f20-unified-audit-repair` (isolated STANDALONE checkout `/Users/seandonahoe/dev/waylandcore-ferrox`; `.git` is a directory; all git ops via `/usr/bin/git`).
- **doc tip (planning only):** `b8a1c9d466427cf87fef28e7fef4be9bc8ba2af5` (tree `c42f430a8c0d660aed01716bdbf466a940c592f5`) — adds ONLY `20-50-SUMMARY.md` over `f0dd5b6d` (`git diff --stat f0dd5b6d b8a1c9d4` = 1 file, `.planning/` only). The seal is the SOURCE-complete HEAD `f0dd5b6d`, not the doc tip.
- **plan base (scope authority):** `b8a1c9d4` (captured before any action; the plan modifies no repository source/test/workflow/lock file).

## Zero Cargo.lock delta — CONFIRMED (no-op re-seal)

Unlike the 20-44 seal (which resynced `Cargo.lock` for a new `wcore-swarm`→`dunce` direct edge), the 20-50 fix touched only two `#![cfg(windows)]`/`windows_impl` files (a containment test + the cmd classifier) and changed NO `Cargo.toml`. The lock is therefore unchanged, confirmed three independent ways:

| Check | Command | Result |
|---|---|---|
| Lock diff vs sealed-but-RED predecessor | `git diff 3f839309 f0dd5b6d -- Cargo.lock` | **EMPTY** (exit 0) — the 20-50 delta introduced no lock change |
| Lock diff through the doc tip | `git diff 3f839309 b8a1c9d4 -- Cargo.lock` | **EMPTY** (exit 0) — no lock change through the planning commits either |
| `--locked` build (would fail on a stale lock) | `remote-cargo.sh f20-51-build … build --workspace --all-features --locked` | **exit 0**, lock NOT reported stale, NOT regenerated, no new crate pulled |

No new/unexpected crate appeared, so no package-legitimacy checkpoint (T-20-51-01) was triggered. There was **nothing to regenerate or commit** — no lock commit was fabricated where there is no delta.

## Gate receipts (Hetzner build farm)

Both gates ran against a **detached checkout of the exact sealed successor** `f0dd5b6d` (tree `ac76c87b`), so the remote-cargo harness's HEAD-tree gate verified the precise sealed candidate tree. Box `root@95.216.244.213:16666`.

| Gate | Command | Result |
|---|---|---|
| `--locked` full build (r10) | `remote-cargo.sh f20-51-build "$(pwd -P)" build --workspace --all-features --locked` | **exit 0** — `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 26s`; slot `/root/cargo-slots/f20-51-build`; committed HEAD `f0dd5b6d`, tree `ac76c87b`. `--locked` succeeded WITHOUT a stale-lock report and WITHOUT regeneration. One warning: pre-existing `imap-proto v0.10.2` future-incompat note (a transitive dependency, NOT our code, NOT a lock change) — out of scope, logged not fixed. |
| Aggregate Linux floor (r4) | `remote-cargo.sh f20-51-aggregate "$(pwd -P)" nextest run --workspace --profile ci --no-fail-fast` | **exit 0** — `Summary [70.263s] 11509 tests run: 11509 passed (3 flaky), 48 skipped` = **11509 / 0 / 48** (0 FAIL, 48 SKIP, 3 known-flaky retried green); slot `/root/cargo-slots/f20-51-aggregate`; committed HEAD `f0dd5b6d`, tree `ac76c87b`. |

**Aggregate identity / "run ID":** nextest's `ci` profile emits no distinct run-id token; the run identity IS the remote receipt tuple — slot `f20-51-aggregate`, committed source HEAD `f0dd5b6d` (tree `ac76c87b`), box `95.216.244.213:16666`, `Summary [70.263s]`. Recorded verbatim rather than fabricating an ID.

The aggregate is **exactly 11509/0/48** — identical to the 20-50 sealed baseline. Both 20-50 edits are `#![cfg(windows)]`/`windows_impl`, so the Linux compiled output is unchanged; this floor proves no Linux regression, NOT the Windows fixes (honesty gate, T-20-51-02 satisfied — the exact count is met, so nothing was rationalized).

## Task Commits

This plan modifies no repository source/test/workflow/lock file. Both tasks are Hetzner receipts against the sealed successor; the only artifact written is this summary.

1. **Task 1: Confirm zero Cargo.lock delta and record the sealed successor SHA** — no code commit (receipt-only: `f20-51-build` --locked build exit 0, zero lock delta confirmed).
2. **Task 2: Prove the sealed successor builds --locked and the aggregate Linux suite is unregressed** — no code commit (receipt-only: `f20-51-build` + `f20-51-aggregate` 11509/0/48).

**Plan metadata:** `<metadata-hash>` (docs: seal further-repaired successor — 20-51-SUMMARY.md only).

## Files Created/Modified

- `.planning/phases/20-transactional-delegated-mutation/20-51-SUMMARY.md` — this seal record (created). No other repository file touched.

## Decisions Made

See `key-decisions` in the frontmatter. In brief: proved against the detached source-complete HEAD `f0dd5b6d` (not the doc tip) so the tree-hash gate binds to `ac76c87b`; confirmed zero lock delta three ways; claimed no requirement (r10/r4 gate-proven on Linux, completion deferred to 20-53/20-56); metadata commit is the summary only.

## Deviations from Plan

None — plan executed exactly as written. The scope-gate verify form requires at least one required-path argument (`$# -ge 2`), so for a zero-source-change plan it is exercised by passing the single committed planning doc as the required path AFTER the metadata commit (proving the entire change-union from the plan base is exactly that one doc — no source/test/workflow/lock file). The substantive zero-source-change proof (plan base `b8a1c9d4` == HEAD before the metadata commit, clean tree, empty `base..HEAD` source diff) held independently.

## Issues Encountered

None. The isolated checkout's known pre-existing `AGENTS.md` ijfw-metadata churn (documented in 20-50) was clean throughout this plan; no restore was needed. No Cargo ran on the Mac.

## User Setup Required

None - no external service configuration required.

## Explicit deferral (native + requirements)

- The two 20-50 Windows-only fixes' **compile AND real-hardware behavior** (non-vacuous captured-PID reap; exact-final-component cmd classifier) are proven ONLY at the **20-53** self-hosted msvc re-dispatch. Neither this Mac nor Hetzner Linux compiles a single line of them.
- No native, and no Phase-20 requirement, is claimed here. REQ-native-r10 (lock consistency) and REQ-native-r4 (Linux no-regression) are green as **gates** on Linux; their completion is bound to 20-53 (native) / 20-56 (terminal).
- **After this seal, NO source, test, or workflow change follows** for the remainder of the further-repaired-successor sequence. The sealed SHA `f0dd5b6d` (tree `ac76c87b`) is the exact candidate all downstream gates bind to.

## Next Phase Readiness

- The sealed successor `f0dd5b6d` (tree `ac76c87b`) is lock-consistent (`--locked` green, zero lock delta) and Linux-unregressed (11509/0/48). Ready for the **20-52** four-way pre-native cross-audit.

## Self-Check: PASSED

- Summary file exists at `.planning/phases/20-transactional-delegated-mutation/20-51-SUMMARY.md`.
- Sealed `source_sha` `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2`, tree `ac76c87b318ee4ba8c34927dea23e40e63fd0776` (verified `f0dd5b6d^{tree}` == `ac76c87b`).
- Zero lock delta verified: `git diff 3f839309 f0dd5b6d -- Cargo.lock` EMPTY; `git diff 3f839309 b8a1c9d4 -- Cargo.lock` EMPTY.
- `f20-51-build` `--locked --workspace --all-features` = exit 0 (`Finished … in 1m 26s`), lock not regenerated, no new crate.
- `f20-51-aggregate` `nextest --profile ci --no-fail-fast` = exit 0, `11509 passed, 0 failed, 48 skipped`.
- No Phase 20 requirement claimed (`requirements-completed: []`).
- No repository source/test/workflow/lock file modified by this plan.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
