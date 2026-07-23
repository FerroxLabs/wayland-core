---
gsd_state_version: 1.0
milestone: v1.0
milestone_name: milestone
current_phase: 20
current_phase_name: transactional-delegated-mutation
status: executing
stopped_at: Plans 20-03, 20-15, 20-04, 20-06, 20-09, 20-10, 20-11, 20-12, and 20-13 complete. 06A candidate seal (20-06/09) + 06B HardContainmentAuthority (20-10/11) both delivered+reviewed. 20-13 (06D) added the 3 black-box proof files (source ace4bd2, scope-ok paths=3): live bwrap hard-containment qualification + descendant no-residue, hostile gate-evidence/durable-replay fail-closed, positive host→AcceptedCandidate. All OWNED gates Linux-green (8 focused pairs, 3 integration binaries, swarm/sandbox --lib, clippy 3-crate -D warnings, fmt --all). The windows-gnu cross-check surfaced a PRE-06D regression (bisected: F20-entry 97e4491 CLEAN → pre-06B 1ace696 RED; root cause 20-02 4dcd62a moved windows_impl a module level deeper leaving stale super:: paths + 2 sibling private-field ctors; dead-on-Linux so all Linux-only proofs incl. 20-02 acceptance missed it) — production windows_impl code OUTSIDE 06D scope + OUTSIDE the 06A-06D audited blobs + OUTSIDE the metadata-only chain, ROUTED TO 20-08 with a VALIDATED fix (scratchpad/windows-impl-modpath-fix-for-2008.patch, proven windows-gnu green out-of-chain, cfg(windows)-only/Linux-neutral). Per f20-14 native Windows is deferred, so not blocking 20-14. NOT a 06D defect. 20-14 = the fresh non-author all-severity independent AUDIT of the exact integrated 06A-06D candidate (06D source ace4bd2/tree 25c2c6c8, base 9d67fdd): zero-finding f20-14 PASS (review bc907b7; checks all_severity/evidence_integrity/integration_authority; deferred native_macos/native_windows; reviewer wayland-f20-14-independent-audit ≠ all source authors; verify-review-pair.sh + verify-review-result.mjs both green). It certified acceptance-authority forgery resistance, the SealedCandidateRoot cwd-from-seal soundness, the AcceptedCandidate seal-before-guard drop order, one-use verify_no_drift, and that the windows_impl regression is out of the audited blobs (deferred to 20-08/UAT). A prior run correctly FAILed at the tree-consistency gate on a mis-recorded 20-13 source_tree (fixed metadata-only: 25c2c6c8; 06D source unchanged). Integrated 06A-06D candidate QUALIFIED; admits 20-05/20-07. 20-05 routed all orchestration callers (Anvil Forge/seat, Council, Crucible, CLI workflow/anvil/crucible) through the workspace-aware production spawner (source a528dbc/tree e8b3768; task base 8321f8e). Scope expanded 10→11 (added spawner/durable_launch.rs) for the authorized new public run-and-retain seam spawn_builder_into_retained_checkout → (SubAgentResult, MutationAttemptGuard): fails closed pre-child on non-IsolatedMutation, allocates one transaction-owned isolated checkout via 20-04 machinery, runs the builder bound to it, returns the still-armed guard instead of terminalizing — no 2nd checkout/global-CWD/parent mutation; preserves audited invariants. Each Forge candidate carries its own opaque guard/identity through gating; winner-only landing (ClimbOutcome), RAII loser cleanup. All owned gates green (scope-ok paths=11; clippy 2-crate -D warnings; anvil_forge_transaction 4/0; crucible_council 15/0; cli --lib 1688/0; fmt). 2 PRE-EXISTING deterministic spawn_tool failures (parent-workspace-not-bound, 20-04-era; spawn_tool.rs/spawner.rs byte-identical base→HEAD; in 20-12/13 serial sets) tracked for the 20-08 aggregate. 20-07 parent landing/CAS complete (source d527ca8/tree 2d8231c; task base 5a30ea8): pure parent-owned worktree/parent.rs quarantined-import → synthesize-successor-parent-side (CandidateSeal binds HEAD==base) → revalidate-from-quarantined-bytes → promote-before-CAS → git update-ref <ref> <new> <old> → coherent HEAD/index/worktree projection; child_transaction/parent.rs authorizes from durable state gated on 06C accepted_candidate; 8 landing SessionEvents + deterministic reducer recovery matrix; all 8 landing authority events on the public-append denylist (scope 8→9 for session_journal.rs). Fail-closed pre-mutation on drift/dirty/non-descendant/foreign/lock/conflict; reverse-CAS rollback never overwrites foreign (RollbackForeignDrift→RecoveryRequired). Independent adversarial security review: clean except 1 MEDIUM (.git/config filter TOCTOU — fixed via seal.revalidate() re-scan before git add + GIT_DIR pinned to parent common dir + regression test) + 1 LOW (denylist test → all 8). Gates green (scope-ok paths=9, clippy 2-crate -D warnings, cas-test 5/0, swarm --lib 99/0, fmt). Next boundary: plan 20-08 (full lifecycle: snapshot→open→revalidate→allocate→launch→gates→land→rollback→cleanup→receipt; + apply windows_impl patch + resolve 2 deterministic spawn_tool failures; the aggregate proof runs after it; depends 20-01/02/03/04/05/07/14). 20-12 built the 06C source packet — parent-owned AuthorizedGateClosureRegistry (SHA-256-sealed closures; unknown/substituted/drifted fail closed pre-spawn) + fail-closed gate state machine (module-private, exact-seal, in-order observed results from a consumed one-use HardContainmentAuthority spawn; live-seal-derived cwd) + opaque guard-owned AcceptedCandidate (owns MOVED still-armed MutationAttemptGuard + CandidateSeal; load-bearing seal-before-guard drop; no pub-ctor/Clone/Serialize) minted only after authoritative durable receipt append/reopen/reduce/match. 8 wcore-agent files (4 new). Linux-proven at source b8a260e: scope-ok paths=8; clippy -p wcore-agent --all-targets --all-features -D warnings clean; ALL 6 hostile process-isolated nextest pairs 1-select/1-unretried-PASS; touched code 71/71 in nextest isolation. The cargo `test --lib` parallel flakiness (20/18 non-deterministic sets, 4 serial) is PRE-EXISTING journal-writer-lease + inherent-parallelism contention in untouched files (engine/session/council/spawn_tool), independently proven across 4 configs — NOT a 20-12 regression (deferred to 20-08 aggregate). 06C makes NO parent-landing/CAS/lifecycle claim. Next boundary: plan 20-13 (depends on 20-12).
last_updated: "2026-07-23T11:00:00.000Z"
last_activity: 2026-07-23
last_activity_desc: "Plan 20-08 complete (source 5e665ec/tree e5dcf77c; task base 21329d0). Full transactional delegated-mutation lifecycle composed (land_selected_winner: open→06C hard-containment gate re-run→20-07 CAS→rollback→receipt) and wired into the live Anvil drive_climb_full: a gate-verified climb winner is re-verified under hard containment and lands onto a Wayland-owned integration clone's refs/heads/<branch>, SURFACE-FOR-ACCEPT (retained clone, Desktop-owned GC per Sean), never touching the user's repository. New: gate_authorization (Anvil gate→06C AuthorizedGate, digest-correct + containment-preserving), reachability/identity seams (session_journal, winner_identity, create_integration_checkout, current_branch). Scope expansions documented (gate_authorization, mutation_workspace pub(crate), LiveCandidateRoot Send+Sync, worktree_manager seams). clippy -p wcore-swarm -p wcore-agent -D warnings clean; anvil_forge_transaction 5/5 (incl. production_landing real-bwrap-gated Landed proof, user tree untouched); transactional_delegated_mutation_test 9/9. THREE independent adversarial reviews CLEAN (terminal composition, gate translation, full integrated path). Open: builder-write→seal TODO (harness, content proven at lower seam); clean TransactionWorkspace::persist() follow-up. Next: plan 20-16 (fresh non-author review of the 20-08 product; deferred native_macos/native_windows)."
progress:
  total_phases: 1
  completed_phases: 0
  total_plans: 18
  completed_plans: 16
---

# Project State

## Project Reference

See: `.planning/PROJECT.md` (updated 2026-07-19)

**Core value:** Deliver a simple, bounded, crash-complete, transactionally delegated, operator-complete cross-platform agent proven through the packaged product.
**Current focus:** Phase 20 — transactional-delegated-mutation

## Current Position

Phase: 20 (transactional-delegated-mutation) — EXECUTING
Plan: 17 of 18 — native path RED/incomplete. 20-17 persisted a still-PENDING native tuple for candidate `6937ef6`; no 20-18 has run against `6937ef6`. Next: native-repair successor (R1–R12) → fresh 20-16 (native NOT deferred) → 20-17 re-prep → 20-18 (Sean-authorized)
Status: Executing Phase 20
Last activity: 2026-07-23 — Whole-program consolidation + 5-agent Phase-20 plan audit. CORRECTED native provenance (a prior same-day edit had overstated it): the only recorded tuple-authorized **20-18 ran RED on 2026-07-22 against the PRE-repair candidate `5e665ec`** (hosted `windows-2022` + ephemeral macOS). Its failures were a wcore-sandbox **Windows COMPILE** error (windows_impl module-path debt, E0425/E0423) + a **macOS broken test-mapping** (`live_integrity.rs` is `#![cfg(windows)]` → 0 tests) — NOT an AppContainer read/token failure. `6937ef6` is the repaired successor that fixed those; 20-16 reviewed it CLEAN (native deferred by design); 20-17 persisted a still-PENDING tuple; **no formal 20-18 has run against `6937ef6`.** A separate **OFF-PLAN diagnostic** on real Windows 11 (SeanDesktop, 2026-07-23; the BRIEF §2 admits it drifted from Ferrox discipline) then found a deeper AppContainer read/token boundary issue: `storage.rs` `.write(true)` is spike-proven+uncommitted, but the `process.rs` "drop deny-only SIDs" fix is **NOT on disk** — the working tree instead holds an invasive diagnostic (full-token swap via `current_token`, `DISABLE_MAX_PRIVILEGE=0`, +90 diag lines) that BREAKS the AppContainer boundary and must be reset to pristine `be84bd2`. Linux `nextest --profile ci` 11509/0 is the reported Hetzner aggregate; Windows + macOS native are UNPROVEN for `6937ef6` on hardware. Next: native-repair successor (R1–R12) → fresh 20-16 (native NOT deferred) → 20-17 re-prep → 20-18 (Sean-authorized)

Progress: [█████████░] 83%

## Execution Authority

- F00-F19 entered Phase 20 at `97e44910fc6dd4761f1f862dbf54a5a76262cef2` (tree `8d27bd96b476a728d3ebbe0e1583c6488dd5effc`). Accepted 20-01 source is `626e1d4d3dee9fee7008ad172ec0b4add8f2004e`; accepted 20-02 source is `96afb30aff362ef8f0d4f6f93773eae548d989ee`.
- The one reconciled source before reopened 20-03 is `94f014d039b8babf3f5926385a3bbc5cb5cf3c41` (tree `49635e1678bd96e42353ab0f7f943ba87497e9d0`). It contains both accepted source lineages, their standard summaries, and the later source repairs in their history. The independently accepted planning successor of `94f014d` is the clean source checkout from which the executor captures `F20_03_EXECUTION_BASE` and tree before Task 1A; no alternate candidate may advance.
- **Current Phase-20 candidate (recorded 2026-07-23):** `6937ef61aa2ad2074dd7875f9cde2369fc104461` (tree `6db6fc859539b43f083aa0a22f3e3e0a014721ae`) — the repaired 20-08 successor, 20-16 fresh independent review CLEAN (native deferred by design). Working commit `be84bd2b9d8a340e85a27533286cc5d14dfae45d` (tree `6d0948406d3d7835f7bd7d37397b77aa744484f4`) on branch `plan/f20-unified-audit-repair` carries a subsequent `AttrListGuard` visibility fix on top. Linux `nextest --profile ci` 11509/0 (Hetzner, reported); **Windows + macOS native are UNPROVEN for `6937ef6` on hardware** (no 20-18 has run against it — the 07-22 RED was against pre-repair `5e665ec`). The native-repair successor derives from this candidate. **Every candidate SHA change invalidates the prior 20-16 review and the prior 20-18 authorization** — `cb6f06bd…` is spent/void; a RED result AND any new candidate SHA each require FRESH Sean authorization.
- Standard GSD execution keeps code, eighteen dependency-ordered plans, and normal summaries in one clean source checkout. Fresh non-author review plans are explicit GSD boundaries; repository-local verifiers reject incomplete scope unions, mutable source blobs, and self-referential summary identities.
- Every plan has nonempty F20 requirement traceability. Source plans produce implementation or hostile-test changes; review plans are fresh-executor, exact-candidate authority boundaries. Later findings route to GSD audit-fix or explicit replanning.
- Node repair, phase auto-advance, worktree mode, and Phase 20 plan parallelization are disabled. Global build and test gates call the committed-HEAD Hetzner remote-cargo harness, never Cargo on this Mac, once after plan 20-08.
- Independent code review, validation, security review, aggregate proof, and authorized native Windows UAT all block `phase.complete` until clear.
- No Cargo on Mac. Authoritative Linux Cargo proof uses `hetzner-dsm:/root/wayland`; native Windows/macOS proof runs at declared gates.
- No push, main merge, issue closure, release, deployment, or canary promotion without Sean.

## Performance Metrics

**Velocity:**

- Total plans completed: 7
- Average duration: Not established
- Total execution time: Not established

## Accumulated Context

### Decisions

- Preserve accepted F00-F19 and inventory/salvage partial F20-F23 work; do not restart the program or treat unadmitted branches as accepted.
- Keep F20 as the current admission gate while planning additive downstream scope.
- Operator completeness and continuous peer comparison are release requirements.
- Split F24-F27 into bounded plans while preserving canonical phase IDs.

### Pending Todos

- Finish independent validation of the unified eighteen-plan Phase 20 graph, reconcile the clean source execution checkout with the two completed summaries, and resume the reopened plan 20-03.
- Classify all 56 linked worktrees before any cleanup or deletion.
- Complete D1 (CTRL-02) linked Desktop plan/consumer replay admission AND pin CTRL-01 Competitive-Ledger Hermes/OpenClaw baselines (with F03/F05 evidence mapped) before broad Phase 21 execution; complete D2 (CTRL-04) by Phase 23 exit.
- Refresh the competitive ledger (CTRL-01) and route live field regressions (CTRL-03) at every admitted phase.

### Blockers/Concerns

- The eighteen replacement plans must pass standard GSD plan checking with complete F20 requirement coverage; review-only boundaries must remain fresh-executor, exact-candidate, and mechanically source-immutable.
- Existing F20 successors have dependency and platform-proof boundaries that must be reconciled into one candidate.
- Local `pwsh` is absent. Before native Windows UAT, prove a pinned execution environment or separately bound authorized host; do not install or dispatch during planning.
- **Native path RED / incomplete (provenance corrected 2026-07-23)** — SUPERSEDES the 2026-07-21 "20-17 BLOCKED / runners offline" note (runners were subsequently used for the 07-22 run). PROVENANCE per on-disk artifacts (`20-18-SUMMARY.md`): the only recorded tuple-authorized **20-18 ran RED 2026-07-22 against PRE-repair `5e665ec`** on hosted `windows-2022` + ephemeral macOS — Windows wcore-sandbox **COMPILE** failure (windows_impl module-path debt: E0425 `reserve_output`/`probe_single_flight`/`BUFFERED_OUTPUT_LIMIT_BYTES`, E0423 private fields) + **macOS test-mapping** failure (`live_integrity.rs` is `#![cfg(windows)]` → 0 tests → `--no-tests=fail`). `6937ef6` (repaired successor) fixed those; 20-16 CLEAN (native deferred); 20-17 persisted a still-pending tuple; **NO 20-18 has run against `6937ef6`.** A separate 2026-07-23 **OFF-PLAN diagnostic** on real Windows 11 (drifted from Ferrox discipline per BRIEF §2) then found a deeper AppContainer read/token boundary issue. Overall root cause: the 20-08 native machinery was authored without ever running on the target OS. Repair (R1–R12 + audit addenda): `storage.rs` `.write(true)` (spike-proven, uncommitted); the REAL `process.rs` "drop deny-only SIDs" fix is **NOT yet written** (tree holds a boundary-breaking diagnostic — full-token swap — to reset to pristine `be84bd2`); `wcore-agent` snapshot.rs import; the wcore-sandbox windows_impl COMPILE debt (claimed fixed in `6937ef6` but never native-verified); 2 test bugs; 2 harness targets wired to Linux-only Bubblewrap tests (real Windows Job-Object tests must be authored); macOS proof-harness mapping; stale `Cargo.lock`; self-hosted-runner `runs-on`; the verifier-only-vs-writer gap in `scripts/f20-native-uat-proof.mjs`. Execute under a plan (native-repair successor), not improvised.

## Deferred Items

| Category | Item | Status | Deferred At |
|----------|------|--------|-------------|
| Cloud breadth | Additional providers beyond one F25 reference backend | Deferred | Initial GSD roadmap |
| Desktop | Presentation and companion-app implementation | Linked plan required | Initial GSD roadmap |

## Session Continuity

Last session: 2026-07-20
Stopped at: Plans 20-03, 20-15, 20-04, 20-06, 20-09 complete (Linux-proven). 20-03 substrate (d343fc72), 20-15 review ZERO-FINDING PASS (repaired a HIGH), 20-04 production spawner (30a2b4f), 20-06 opaque live CandidateSeal source packet (source 10d7573; clippy clean + 92 wcore-swarm/60 wcore-sandbox tests), 20-09 independent non-author review of 20-06A = zero-finding f20-09 PASS (review 8d66277; 3 adversarial rounds repaired real .git/mode false-PASS gaps) → 06A qualified. Next: plan 20-10 (depends on 20-06/06A), then dependency order 11,12,13,14,05,07,08,16,17,18; review gates at 11 (rev 06B)/16 (rev 08) + audit 14; Sean hard-stop at 20-18.
Resume file: None
