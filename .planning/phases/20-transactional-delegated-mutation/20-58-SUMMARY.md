---
phase: 20-transactional-delegated-mutation
plan: "58"
subsystem: infra
tags: [native-repair, re-seal, zero-lock-delta, locked-build, linux-no-regression-floor, honest-deferral, further-repair-successor, sealed-candidate]

# Dependency graph
requires:
  - phase: "20-57"
    provides: "The further-repaired successor delta 8a1d2d84 (tree c1fe79fe) over the 20-52-blocked f0dd5b6d (tree ac76c87b): ALL THREE host-side query observers in crates/wcore-sandbox/tests/hard_process_containment_windows.rs hardened to FAIL CLOSED in one comprehensive pass. Changed ONE #![cfg(windows)] test file only — NO Cargo.toml, NO Cargo.lock, so no manifest/lock change."
provides:
  - "SEALED further-repaired-successor candidate source_sha 8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e (tree c1fe79fe4a6d68a536078be4887343a82b5fce38) — the ONE exact SHA every downstream gate (20-59 four-way pre-native cross-audit, then rebound 20-53 native re-dispatch, 20-54 native-inclusive review, 20-55 prep, 20-56 terminal) binds to. After this seal, NO source/test/workflow change follows."
  - "ZERO Cargo.lock delta CONFIRMED: unlike the 20-44 seal (which resynced the lock for a new wcore-swarm→dunce direct edge), the 20-57 fix changed no Cargo.toml (one #![cfg(windows)] test file only), so the lock is byte-identical to the 20-52-blocked predecessor f0dd5b6d. git diff f0dd5b6d 8a1d2d84 -- Cargo.lock is EMPTY; git diff 8a1d2d84 fe4432cf(HEAD) -- Cargo.lock is EMPTY. A --locked --workspace --all-features Hetzner build against a DETACHED checkout of the exact tree c1fe79fe succeeded (exit 0) WITHOUT a stale-lock report and WITHOUT regenerating the lock — no new crate pulled (REQ-native-r10 gate-proven; requirement NOT claimed)."
  - "LINUX NO-REGRESSION floor green on the sealed successor: Hetzner aggregate nextest --workspace --profile ci --no-fail-fast = 11509 passed / 0 failed / 48 skipped (exit 0). The 20-57 edit is Windows-only (#![cfg(windows)]), so Linux compiled output is unchanged — the count is identical to the 20-57 and 20-51 baselines, an honesty gate not proof of the Windows fix (REQ-native-r4 gate-proven; requirement NOT claimed)."
affects: ["20-59", "20-53", "20-54", "20-55", "20-56"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "No-op-lock re-seal: when the antecedent fix changes no Cargo.toml, confirm zero lock delta by proving a --locked --workspace --all-features build against a DETACHED checkout of the exact source-complete HEAD succeeds without the lock being reported stale or regenerated — the lock counterpart of the 20-44 dunce-edge resync, inverted to a confirmation-of-no-change. Fabricating a lock commit when there is no delta is forbidden."
    - "Anchor the sealed candidate by DETACHING HEAD to the source-complete SHA (8a1d2d84, tree c1fe79fe) before invoking the remote-cargo harness, so the harness's HEAD-tree gate binds to the precise sealed candidate tree rather than the doc-tip tree (fe4432cf/d4e74404, which differs only by .planning/20-57-SUMMARY.md). After the two gates, HEAD is returned to the branch."

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-58-SUMMARY.md
  modified: []

key-decisions:
  - "Proved against a DETACHED checkout of the exact 20-57 source-complete HEAD 8a1d2d84 (tree c1fe79fe), NOT the branch doc-tip fe4432cf (tree d4e74404). The two trees differ ONLY by the 20-57-SUMMARY.md planning doc (git diff --name-only 8a1d2d84 fe4432cf = 1 file, .planning/ only), which does not affect the build — but the remote-cargo harness hard-verifies the shipped HEAD tree equals the local HEAD tree, so detaching to 8a1d2d84 makes both Hetzner gates bind to the precise sealed candidate tree c1fe79fe. After the two gates, HEAD was returned to the branch plan/f20-unified-audit-repair."
  - "ZERO lock delta confirmed three ways: (1) git diff f0dd5b6d 8a1d2d84 -- Cargo.lock is EMPTY (the 20-57 fix introduced no lock change); (2) git diff 8a1d2d84 fe4432cf -- Cargo.lock is EMPTY (no lock change through the doc tip either); (3) the --locked --workspace --all-features Hetzner build succeeded (exit 0) WITHOUT the lock being reported stale and WITHOUT regeneration — a stale/incoherent lock would have failed --locked before compiling. No new crate appeared, so no package-legitimacy checkpoint was triggered. This is the no-op counterpart of the 20-44 lock resync."
  - "A re-seal after a manifest-neutral fix is a CONFIRMATION, not a mutation: run the --locked all-features build to prove the lock is coherent+unchanged, run the aggregate to prove the Linux floor is unregressed, then record the source-complete HEAD verbatim as the sealed candidate. This plan writes only the summary; no repository source, test, workflow, or lock file was modified (scope gate: scope-ok, paths=1)."
  - "HONEST deferral (CONTEXT D5/D7): the 20-57 fix lives in a #![cfg(windows)] file — NEITHER the Mac NOR Hetzner Linux compiles a line of it. This seal's Hetzner --locked build + 11509/0/48 aggregate are a NO-REGRESSION FLOOR + lock-consistency proof only, NOT proof of the Windows observer fix. The observer fix's Windows compile AND real-hardware fail-closed behavior are proven ONLY at the rebound 20-53 native msvc re-dispatch. No Windows fix and no native claim are made here."
  - "requirements-completed is [] — NO Phase-20 requirement is claimed. REQ-native-r10 (lock consistency) and REQ-native-r4 (Linux no-regression floor) are gate-PROVEN by the two Hetzner receipts, but per the further-repaired-successor chain the requirement completion is deferred to the terminal 20-56 after the native run; this seal only anchors the candidate. Mirrors the 20-51 re-seal exactly."
  - "Chain position: 20-57 (fix) → 20-58 (this re-seal) → 20-59 (four-way pre-native cross-audit) → (rebound) 20-53 native re-dispatch → 20-54 → 20-55 → 20-56. Do NOT mutate the executed/sealed records 20-43/44/45/50/51/52/57."

patterns-established:
  - "The manifest-neutral re-seal is a confirmation gate: DETACH to the source-complete SHA, run --locked all-features (proves lock coherent + unchanged, exit-0-before-compile is the stale-lock tripwire), run the aggregate (proves the Linux floor), record the SHA verbatim. No lock commit is fabricated when the delta is zero."

requirements-completed: []  # NO Phase-20 requirement claimed. r10/r4 are gate-proven by the two Hetzner receipts; completion deferred to the terminal 20-56 after the native run.

# Coverage metadata (#1602)
coverage:
  - id: D1
    description: "ZERO Cargo.lock delta confirmed and the sealed successor builds --locked --workspace --all-features on Hetzner against the exact tree c1fe79fe (lock coherent + unchanged, no new crate). The 20-57 fix changed no Cargo.toml, so this is a no-op-lock re-seal — no lock commit fabricated (REQ-native-r10 gate-proven; requirement NOT claimed)."
    requirement: "REQ-native-r10"
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-58-build <detached 8a1d2d84 / tree c1fe79fe> build --workspace --all-features --locked => exit 0, Finished dev profile in 1m 27s, no stale-lock report, no regeneration, no new crate"
        status: pass
      - kind: unit
        ref: "git diff f0dd5b6d 8a1d2d84 -- Cargo.lock = EMPTY; git diff 8a1d2d84 fe4432cf -- Cargo.lock = EMPTY; no Cargo.toml change f0dd5b6d..8a1d2d84"
        status: pass
    human_judgment: false
  - id: D2
    description: "Linux no-regression floor: the sealed successor passes the aggregate nextest --workspace --profile ci --no-fail-fast at 11509/0/48 (Windows-only 20-57 edit => unchanged Linux compiled output). Honesty gate, not proof of the Windows fix (REQ-native-r4 gate-proven; requirement NOT claimed)."
    requirement: "REQ-native-r4"
    verification:
      - kind: integration
        ref: "remote-cargo.sh f20-58-aggregate <detached 8a1d2d84 / tree c1fe79fe> nextest run --workspace --profile ci --no-fail-fast => exit 0, Summary [70.465s] 11509 tests run: 11509 passed (1 flaky), 48 skipped = 11509/0/48"
        status: pass
    human_judgment: false
  - id: D3
    description: "The Windows observer fail-closed fix's compile + real-hardware behavior — deferred to the rebound 20-53 native msvc re-dispatch. NEITHER Mac NOR Hetzner Linux compiles the #![cfg(windows)] file; this seal proves nothing about the Windows fix."
    verification:
      - kind: manual_procedural
        ref: "deferred to rebound 20-53 native msvc self-hosted runner (Sean-gated infra)"
        status: unknown
    human_judgment: true
    rationale: "The observer fix is #![cfg(windows)] and is NOT compiled or executed by any pre-dispatch Mac/Linux gate; only the self-hosted msvc runner at 20-53 can prove its compile and fail-closed behavior. This seal is a no-regression floor + lock-consistency proof, not proof of the Windows fix."

# Metrics
duration: ~15min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 58: Re-seal the further-repaired successor 8a1d2d84 — zero-lock-delta confirmation + 11509/0/48 Linux floor Summary

**The 20-57 fix (fail-closed hardening of all three host-side reap-observers) changed ONE `#![cfg(windows)]` test file and NO manifest, so this is a no-op-lock re-seal: a `--locked --workspace --all-features` Hetzner build against a DETACHED checkout of the exact 20-57 source-complete HEAD `8a1d2d84` (tree `c1fe79fe`) succeeded (exit 0) with the lock UNCHANGED — no stale-lock report, no regeneration, no new crate — and the aggregate `nextest --workspace --profile ci --no-fail-fast` returned exactly 11509 passed / 0 failed / 48 skipped, the Linux no-regression floor. The sealed further-repaired-successor candidate is `source_sha 8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e` / tree `c1fe79fe4a6d68a536078be4887343a82b5fce38` — the ONE exact SHA every downstream gate (20-59, then rebound 20-53…20-56) binds to. After this seal, NO source/test/workflow change follows. No Phase-20 requirement is claimed; the Windows observer fix's behavior is proven only at the rebound 20-53 native re-dispatch.**

## Performance

- **Duration:** ~15 min (two cold Hetzner slots)
- **Completed:** 2026-07-24
- **Tasks:** 2
- **Files modified:** 0 repository source/test/workflow/lock files (this plan writes only this summary)

## Sealed candidate identity

| Field | Value |
|-------|-------|
| Sealed successor commit (`source_sha`) | `8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e` |
| Sealed successor tree | `c1fe79fe4a6d68a536078be4887343a82b5fce38` |
| Predecessor (20-52-blocked, sealed by 20-51) | `f0dd5b6d312af616f268f96f34c3bc9fc962c4d2` (tree `ac76c87b318ee4ba8c34927dea23e40e63fd0776`) |
| Branch | `plan/f20-unified-audit-repair` (standalone checkout `/Users/seandonahoe/dev/waylandcore-ferrox`, `.git` = directory) |
| Branch doc-tip at seal time | `fe4432cf08e9b0017a2e9b9cc19228ff18205833` (tree `d4e744046f1c0e2723edd977ed1d16b682cfe02c`) — differs from the sealed tree ONLY by `.planning/…/20-57-SUMMARY.md` |
| Cargo.lock delta introduced by 20-57 | **NONE** (no `Cargo.toml` change; one `#![cfg(windows)]` test file only) |

## Zero lock delta — confirmed three ways

1. `git diff f0dd5b6d 8a1d2d84 -- Cargo.lock` → **EMPTY** (the 20-57 fix introduced no lock change over the sealed predecessor).
2. `git diff 8a1d2d84 fe4432cf -- Cargo.lock` → **EMPTY** (no lock change through the doc tip either).
3. `git diff --name-only f0dd5b6d 8a1d2d84 -- '*Cargo.toml' 'Cargo.lock'` → **EMPTY** (no manifest/lock file touched).
4. The `--locked --workspace --all-features` Hetzner build **succeeded (exit 0)** WITHOUT the lock being reported stale and WITHOUT regeneration — `cargo` proceeded straight to `Compiling` (a stale/incoherent lock under `--locked` errors BEFORE any compilation). No new crate appeared, so no package-legitimacy checkpoint was triggered.

This is the no-op counterpart of the 20-44 dunce-edge lock resync: because the antecedent fix changed no `Cargo.toml`, the correct action is to CONFIRM the lock is unchanged, not to regenerate or fabricate a lock commit.

## Gate receipts (Hetzner, box `root@95.216.244.213:16666`)

Both gates ran against a **DETACHED** checkout of the exact sealed candidate `8a1d2d84` (tree `c1fe79fe`), so the remote-cargo harness's HEAD-tree gate verified the precise sealed tree (`WAYLAND_BUILD_SOURCE_SHA=8a1d2d84…`, remote tree == local HEAD tree `c1fe79fe`, else the harness fails closed before `cargo` runs). HEAD was returned to `plan/f20-unified-audit-repair` (`fe4432cf`) afterward.

| Gate | Slot | Command | Result |
|------|------|---------|--------|
| `--locked` all-features build | `f20-58-build` (`/root/cargo-slots/f20-58-build`) | `cargo build --workspace --all-features --locked` | **exit 0** — `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 27s`; no stale-lock report, no lock regeneration, no new crate; lone benign warning is the pre-existing `imap-proto v0.10.2` future-incompat note (unrelated) |
| Aggregate Linux proof | `f20-58-aggregate` (`/root/cargo-slots/f20-58-aggregate`) | `cargo nextest run --workspace --profile ci --no-fail-fast` | **exit 0** — `Summary [70.465s] 11509 tests run: 11509 passed (1 flaky), 48 skipped` = **11509/0/48** |

The one flaky (`wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream`, FLAKY 2/3) passed on nextest retry; **0 failed**. The flaky COUNT differs from 20-57's run (2 flaky) — that is retry non-determinism, not a regression; the pass/fail/skip totals are identical (11509/0/48), matching the 20-57 and 20-51 baselines exactly. Because the 20-57 edit is Windows-only, the Linux compiled output is unchanged, so this exact count is the required honesty-gate floor — met.

## Scope integrity

- Scope base captured at `fe4432cf` (tree `d4e74404`, generation `g-09548ba3…`) before any action.
- Final scope gate: `bash .planning/scripts/verify-task-scope.sh <gsd-task-base-20-58> .planning/phases/20-transactional-delegated-mutation/20-58-SUMMARY.md` → **`scope-ok … paths=1`** — the ONLY change in the complete TASK_BASE scope union is this summary. No repository source, test, workflow, or lock file was modified.

## Explicit deferrals (honesty gate)

- **The Windows observer fail-closed fix's compile + real-hardware behavior** is deferred to the rebound **20-53** native msvc re-dispatch. NEITHER the Mac NOR Hetzner Linux compiles a line of the `#![cfg(windows)]` change; this seal proves lock consistency + the Linux no-regression floor ONLY, not the Windows fix.
- **No Phase-20 requirement is completed** (`requirements-completed: []`). REQ-native-r10 and REQ-native-r4 are gate-proven by the two receipts but their completion is deferred to the terminal **20-56** after the native run, per the further-repaired-successor chain.

## Task Commits

This plan modifies no repository source and creates commits only for the planning summary (per GSD docs-commit policy).

1. **Task 1: Confirm zero Cargo.lock delta and record the sealed successor SHA** — no source commit; `--locked` Hetzner build receipt (slot `f20-58-build`, exit 0) + three-way zero-lock-delta confirmation.
2. **Task 2: Prove the sealed successor builds --locked and the aggregate Linux suite is unregressed** — no source commit; `--locked` build + aggregate `nextest --profile ci` = 11509/0/48 (slot `f20-58-aggregate`, exit 0).

## Files Created/Modified
- `.planning/phases/20-transactional-delegated-mutation/20-58-SUMMARY.md` — this summary (only file changed).

## Decisions Made
See `key-decisions` frontmatter. In brief: detach to the exact source-complete SHA so the harness tree-gate binds to the sealed tree; confirm (never fabricate) the zero lock delta; record the SHA verbatim; claim no requirement; defer the Windows fix to 20-53.

## Deviations from Plan

None — plan executed exactly as written. The working tree was already clean at capture time (no `AGENTS.md` ijfw-memory restore was needed). HEAD was detached to `8a1d2d84` for the two Hetzner gates and returned to `plan/f20-unified-audit-repair` afterward, exactly as the 20-51 re-seal established.

## Issues Encountered
None. Both Hetzner slots were cold (fresh slot names) and each paid one clean build; sccache shared artifacts across slots. The `--locked` build reached `Compiling` immediately, confirming the lock is coherent.

## Next Phase Readiness
- The sealed further-repaired successor `8a1d2d84` (tree `c1fe79fe`) is anchored and lock-consistent, ready for the four-way pre-native cross-audit (**20-59**), which then rebinds **20-53…20-56**.
- No source/test/workflow/lock change follows this seal. The Windows observer fix's native proof remains deferred to the rebound **20-53** msvc re-dispatch (Sean-gated self-hosted runner).

## Self-Check: PASSED

- `.planning/phases/20-transactional-delegated-mutation/20-58-SUMMARY.md` — created.
- Sealed `source_sha 8a1d2d84403c77a11c4cf6807b8bbafaaf5e8b8e` / tree `c1fe79fe4a6d68a536078be4887343a82b5fce38` exists on `plan/f20-unified-audit-repair` (commit `8a1d2d84`, tree `c1fe79fe`).
- Hetzner receipts: `f20-58-build` exit 0 (`--locked`, Finished in 1m 27s, zero lock delta); `f20-58-aggregate` exit 0 (11509/0/48).
- Scope gate: `scope-ok … paths=1` (only this summary changed; no source/test/workflow/lock drift).

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
