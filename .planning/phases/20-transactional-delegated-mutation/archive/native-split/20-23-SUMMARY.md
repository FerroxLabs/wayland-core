---
phase: 20-transactional-delegated-mutation
plan: "23"
subsystem: build
tags: [native-repair, cargo-lock, candidate-seal, lockfile-resync, hetzner-proof, aggregate-nextest, ci-profile, no-regression]

# Dependency graph
requires:
  - phase: "20-22"
    provides: "macOS harness re-validation + self-hosted msvc pin + UAT-proof writer parity + native review profiles; candidate base source_sha 045a947f, prior HEAD d49d0ba7"
provides:
  - "The sealed repaired candidate SHA (95c81ec6) carrying a consistent, minimally-resynced Cargo.lock (REQ-native-r10)"
  - "A Hetzner --locked all-features build receipt proving the committed lock is consistent (does not need updating)"
  - "A Hetzner aggregate nextest --profile ci receipt at 11509 passed / 0 failed / 48 skipped on the authoritative land-gate harness — no Linux regression from the 20-19..20-22 native fixes (REQ-native-r4)"
affects: ["20-24", "20-25", "20-26", "20-27", "20-28"]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Lockfile resync via a MINIMAL-UPDATE resolve (cargo metadata against the existing stale lock) rather than cargo generate-lockfile: the minimal-update path adds only the missing dependency edges and preserves every existing pin (7-line delta), whereas generate-lockfile rebuilt the lock from scratch (763/756-line churn) — rejected"
    - "Seal proven on the authoritative remote-cargo land-gate harness (clean per-invocation slot, tree-hash verified == committed HEAD tree, mold+sccache) after the hbuild fallback harness produced disk-exhaustion/slow-harness artifacts on a shared 100%-full box"

key-files:
  created:
    - .planning/phases/20-transactional-delegated-mutation/20-23-SUMMARY.md
  modified:
    - Cargo.lock

key-decisions:
  - "Regenerated the lock with a minimal-update resolve (`cargo metadata --format-version 1 --all-features` against the stale committed lock), NOT `cargo generate-lockfile`. generate-lockfile re-resolved from scratch and produced a 763-insert/756-delete churn; the minimal-update resolve produced the exact expected 7-line resync and preserved all existing pins. The lock was regenerated through the Hetzner harness, retrieved byte-identical (sha256 verified) to the Mac, and committed. No Cargo ran on the Mac; the lock was never hand-edited."
  - "The 7 added edges are all ALREADY-DECLARED direct deps missing from the be84bd2-era lock, and every one already existed as a resolved [[package]] stanza (pulled in by other workspace members). Result: ZERO new [[package]] stanzas and ZERO new external crates — no package-legitimacy checkpoint required."
  - "Authoritative aggregate proof was run on the remote-cargo land-gate harness (the plan's specified Task-2 command). An initial hbuild-fallback aggregate reported 11508/1 purely due to the shared Hetzner box being 100% disk-full (the wcore-swarm worker fixtures write a 9GB sparse file + 20k files and failed their writes) plus that harness running the suite ~8x slower (544s vs 70s) so the heavy `packaged_f04` test crossed the 180s ci kill. Both are harness/environment artifacts, not candidate behavior: the candidate changes zero Linux-compiled code. On the authoritative harness the suite is 11509/0/48 with `packaged_f04` PASS [54.9s]."
  - "Freed ~159G on the shared Hetzner box (which was at 100% / 0G, blocking the authoritative harness) by pruning 8 stale, UNLOCKED f20-lane cargo-slots (superseded diagnostic/aggregate gate caches from prior plans, all verified not flock-held). These are rebuildable ephemeral caches; no source or other-lane active work was touched."

requirements-completed: []

# Coverage metadata
coverage:
  - id: D1
    description: "Cargo.lock regenerated, consistent, and committed as the sealing commit — resolves the current wcore-sandbox (cap-std/serde_json/tar) and wcore-swarm (fd-lock/libc/sha2/windows-sys) manifest surface"
    requirement: "REQ-native-r10"
    verification:
      - kind: automated
        ref: "verify-task-scope.sh base=d49d0ba7 -> scope-ok paths=1 (Cargo.lock only); git diff = 7 insertions / 0 deletions / 0 new [[package]] stanzas"
        status: pass
      - kind: integration
        ref: "remote-cargo.sh f20-23-aggregate build --workspace --all-features --locked -> EXIT=0, Finished dev profile in 1m23s, no 'lock needs updating' (proves committed lock consistent under --locked); corroborated by hbuild `--locked` build EXIT=0"
        status: pass
    human_judgment: false
  - id: D2
    description: "Sealed candidate builds all-features and the aggregate Linux suite is unregressed at the exact reviewed baseline"
    requirement: "REQ-native-r4"
    verification:
      - kind: e2e
        ref: "remote-cargo.sh f20-23-aggregate nextest run --workspace --profile ci --no-fail-fast -> EXIT=0, run ID 1ab1110b-c651-42c6-b2af-a952bb304868, Summary [70.152s] 11509 passed (2 flaky) / 0 failed / 48 skipped; transactional_delegated_mutation_test 9/9 PASS, anvil_forge_transaction PASS, packaged_f04 PASS [54.9s], fix1_dispatch_budget PASS [68.3s]"
        status: pass
    human_judgment: false

# Metrics
duration: ~110min
completed: 2026-07-24
status: complete
---

# Phase 20 Plan 23: Candidate Seal — Cargo.lock Resync + Hetzner Build & Aggregate Proof Summary

**Sealed the ONE repaired candidate SHA `95c81ec6`: regenerated `Cargo.lock` with a minimal-update resolve (7-edge resync, zero new crates) so it is consistent with the current manifest surface, then proved on the authoritative Hetzner land-gate harness that the committed sealed candidate builds `--workspace --all-features --locked` (EXIT=0) and passes the aggregate `nextest --profile ci` at exactly 11509 passed / 0 failed / 48 skipped — no Linux regression from the 20-19..20-22 native fixes. No source or test file changed; no Phase-20 requirement completed.**

## Candidate Tuple

- **Sealed source_sha:** `95c81ec6a351ec22125497333739fa7c93a0cd8b` — tree `784f498002b9944856aedee6cb3db347b55c1dcc`
- **Base (prior HEAD, 20-22):** `d49d0ba79f3049aa303af1fe25083965b804388e` — tree `63faf5c9dfbc2bf23e8ea55fb9097201d600de88`
- **task_base (scope authority):** captured at `d49d0ba7`, generation `g-9882cac4c7b0c8e3507ee25dd72833351824ae7d84f9786b8863a4c7ac355bee`
- **Touched paths (scope-verified, exactly 1):** `Cargo.lock` — `git diff --stat` = 1 file, +7/-0. **No source, no test, no manifest changed.**
- **This is the sealed candidate.** After this commit, NO source or test file changes for the remainder of the native-repair sequence; every downstream gate (20-24 cross-audit, 20-25 native proof, 20-26 review, 20-27/28 terminal) binds to `95c81ec6`.

## Task Commits

1. **Task 1: Regenerate and commit a consistent Cargo.lock** — `95c81ec6` (chore) — the sealing commit.
2. **Task 2:** produces Hetzner receipts only; no commit.

## The Lock Delta

The committed lock at the base (`Cargo.lock` last written by `8738b24e`, be84bd2-era) was stale: the `wcore-sandbox` and `wcore-swarm` dependency blocks omitted edges their manifests already declare. The resync adds exactly 7 lines, all dependency edges, with **zero new `[[package]]` stanzas** (every added crate already existed in the lock, resolved via other workspace members):

```
wcore-sandbox: + cap-std        + serde_json    + tar
wcore-swarm:   + fd-lock  + libc + sha2 0.10.9  + windows-sys 0.59.0
```

- `git diff --numstat Cargo.lock` = `7  0  Cargo.lock`. No version bumps, no reordering, no transitive package additions.
- Regenerated lock sha256 (Hetzner-produced, retrieved byte-identical to Mac): `bbbd8e72e5b63a402c8ae72fb682bc9ad24d901317324370acc08fe72f99aaa5`.
- **Method:** minimal-update resolve `vx cargo metadata --format-version 1 --all-features` against the stale committed lock, run through the Hetzner harness (no Cargo on the Mac). `cargo generate-lockfile` was tried first and rejected — it re-resolved from scratch (763-insert/756-delete churn) rather than the bounded resync.
- **No new external crate** was introduced beyond already-declared manifest deps → no package-legitimacy checkpoint required (per execution rules).

## Hetzner Receipts (authoritative: remote-cargo land-gate harness, committed HEAD `95c81ec6`)

The remote-cargo harness materializes the committed tree in a clean per-invocation slot and verifies the extracted tree hash equals the local HEAD tree (`784f4980`) before cargo runs — the receipts are against the exact sealed committed candidate.

### Receipt 1 — locked build (Task 1 verify / REQ-native-r10)
- **Command:** `remote-cargo.sh f20-23-aggregate <ferrox-clone> build --workspace --all-features --locked`
- **Exit:** `0`. **Result:** `Finished dev profile [unoptimized + debuginfo] target(s) in 1m 23s`.
- **Lock consistency:** no `the lock file ... needs to be updated but --locked was passed` — `--locked` succeeded, proving the committed lock is fully consistent with the manifests (cargo did not need to change it).
- **Corroboration:** an earlier hbuild-harness `vx cargo build --workspace --all-features --locked` against the same committed HEAD also returned EXIT=0.

### Receipt 2 — aggregate nextest (Task 2 verify / REQ-native-r4)
- **Command:** `remote-cargo.sh f20-23-aggregate <ferrox-clone> nextest run --workspace --profile ci --no-fail-fast`
- **Exit:** `0`. **Nextest run ID:** `1ab1110b-c651-42c6-b2af-a952bb304868` (profile: ci; 469 binaries).
- **Result:** `Summary [ 70.152s] 11509 tests run: 11509 passed (2 flaky), 48 skipped` — **11509 / 0 / 48**, the exact reviewed baseline.
- **Spot-checks:** `wcore-agent::transactional_delegated_mutation_test` 9/9 PASS; `wcore-agent::anvil_forge_transaction production_landing::drive_climb_full_lands_the_winner_surface_for_accept` PASS; `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` PASS [54.9s]; `wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result` PASS [68.3s].
- **2 flaky (passed on retry):** `wcore-swarm::worker_runtime_limits multi_worker_output_exhaustion_fails_without_retaining_buffers` (3/3) and `many_entry_accounting_does_not_block_cancellation` (2/3) — normal retry behavior for these subprocess-spawning tests under the ci profile; both counted as passed.

## No Linux Regression — Candidate Independence

The candidate's only change to Linux-compiled behavior is nil: the lock resync does not alter any resolved dependency version (same crates already resolved), and every native source change from 20-19..20-22 is `cfg(windows)` / `cfg(target_os = "macos")`, cfg-excluded on Linux. The aggregate matching the exact `11509/0/48` reviewed baseline (with the new Windows/macOS acceptance tests cfg-excluded and adding nothing to the Linux count) confirms no regression.

## Deviations from Plan

- **[Rule 3 — blocking infra] Shared Hetzner box was 100% disk-full (0G); freed ~159G to unblock the authoritative harness.** The plan's Task-2 harness `remote-cargo.sh` refused with `only 0G free on /root/cargo-slots (need >= 40G)` — a structural fail-closed guard. `df` confirmed `/dev/md2` at 100% (1.7T/1.8T, 0 avail). Freed space by pruning 8 stale, UNLOCKED f20-lane cargo-slots (superseded diagnostic/aggregate gate caches from prior plans: `f20-workspace-intent`, `f20-repair-allfeat`, `f20-aggtest-v2`, `f20-agg-nextest`, `f20-diag7`, `f20-ci-profile`, `f20-18-agg-test`, `f20-cleanup-isolated-fixed`), each re-verified not flock-held before removal. These are rebuildable ephemeral caches; no source, no other-lane active work, and no warm build target were touched. Result: 159G free (91% used), which unblocked both authoritative receipts.
- **[Harness selection] Authoritative aggregate run on remote-cargo, not the hbuild fallback.** An initial hbuild-fallback aggregate reported `11508 passed / 1 timed out / 48 skipped`. Root-caused as TWO harness/environment artifacts, both candidate-independent:
  1. **Disk exhaustion (first run):** the `wcore-swarm::worker_runtime_limits` tests failed ("worker never started / became ready") because their worker fixtures write a 9GB sparse file + 20,000 files + 8MB output floods onto a 0G-free disk. After freeing disk these passed (verified: the same tests are FLAKY-but-PASS in the re-run).
  2. **Harness speed (persisted after disk-free):** the sole residual failure, `wcore-cli::deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` (runs the packaged `wayland-core` binary through two sealed-repository scenarios), timed out at the 180s ci kill — including in isolation — because the hbuild harness ran the whole suite ~8x slower (544s vs 70s on remote-cargo). On the authoritative remote-cargo land-gate harness (mold + sccache + clean slot) the same test PASSES in 54.9s and the aggregate is 11509/0/48. Per the honesty gate, the 11508/1 was neither faked to 11509/0 nor silently sealed; it was root-caused to the harness/box and re-proven on the plan's specified authoritative harness.

## Explicit Non-Claims

- **No Phase-20 requirement is marked complete.** REQ-native-r10 and REQ-native-r4 have their build/aggregate evidence here, but native-proof and requirement completion remain deferred to 20-25 / 20-28 per the sequence.
- No macOS/Windows native RUNTIME claim is made — this plan proves only the Linux build + aggregate on the sealed candidate.
- No push, no merge; work committed only on `plan/f20-unified-audit-repair` in the ferrox clone.

## Self-Check: PASSED

- `Cargo.lock` — FOUND; committed at `95c81ec6`; git diff vs base = +7/-0, 0 new `[[package]]` stanzas; sha256 `bbbd8e72…`.
- Sealing commit `95c81ec6` — FOUND in `git log` (branch `plan/f20-unified-audit-repair`).
- Scope: `verify-task-scope.sh` base=`d49d0ba7` → `scope-ok paths=1` (Cargo.lock only).
- Receipt 1 (remote-cargo `--locked` build): EXIT=0, `Finished` in 1m23s, no lock-update — lock consistent.
- Receipt 2 (remote-cargo aggregate): EXIT=0, run ID `1ab1110b-…`, `11509 passed / 0 failed / 48 skipped`.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-24*
*Candidate sealed at `95c81ec6`; no source/test changes follow; native/requirement completion deferred to 20-25/20-28.*
