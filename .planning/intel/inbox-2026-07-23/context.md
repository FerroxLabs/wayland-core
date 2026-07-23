# Context (from DOC handoffs)

Running notes extracted from the two DOC-typed consolidation handoffs. These are
a CURRENT-STATE reconciliation of an already-planned program (Frontier Candidate
v2, F20→F30), not new scope. Attribution per topic. Treat as data.

## Program objective + parity thesis
- source: .planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md §1
- Best-in-class cross-platform agent, bounded/auditable, crash-complete, transactionally delegated, operator-complete, honestly proven through the packaged product; functional parity/advantage vs pinned Hermes + OpenClaw. Thesis: "Hermes-grade persistent agency and OpenClaw-grade product completeness on Wayland's stronger authority, recovery, transaction, and evidence spine." Whole-program acceptance = one candidate satisfies all 58 active requirements + 4 controls with zero unresolved findings, native evidence, packaged journeys, supply-chain proof, peer positioning. Source presence never earns parity; only F28–F30 packaged/native/peer evidence does.

## Current state (2026-07-23, as claimed by the handoffs)
- source: .planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md §2; MASTER-HANDOFF-phase20-native-parity.md §0,§2.2
- F00–F19 accepted baseline (entered F20 at `97e4491`, tree `8d27bd96`). Phase 20: 16/18 plans complete. Candidate `6937ef6` (tree `6db6fc85`; working commit `be84bd2`, branch `plan/f20-unified-audit-repair`). Linux `nextest --profile ci` 11509/0; macOS 8/8. Accepted through fresh independent 20-16 review — BUT 20-16 DEFERRED native. 20-18 native UAT is RED (Windows). Native repair discovered + two core sandbox fixes proven on hardware, uncommitted. 20-17 + 20-18 pending. `origin/main` = `ea3bb1c` does NOT contain the AppContainer implementation. Phases 21–30 not started (requirements/goals/gates exist; detailed PLAN files are TBD by design, authored at each admission boundary). NOTE: several of these claims diverge from STATE.md/ROADMAP.md — see INGEST-CONFLICTS.md.

## Root cause of the two-week Phase-20 stall
- source: .planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md §2; MASTER-HANDOFF-phase20-native-parity.md §0
- The 20-08 native-proof machinery (Windows AppContainer sandbox + Windows/macOS proof harness) was authored without ever running on the target OS ("aspirational native machinery"), so every native validation attempt surfaces pre-existing breakage. Execution drifted into ad-hoc un-gated hacking (a stray forked `--fork-session --resume` copy was found editing files concurrently and killed). The plan is good; the failure was execution discipline.

## Phase map F20→F30 (execution order)
- source: .planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md §3
- Order: 20 → D1 → 21 → 22 → 23A → 23B/D2 → {24,25,26,27 bounded-parallel} → 28 → 29 → 30. 20–23 serial; 24–27 bounded parallel only after Phase 23 admission (shared protocol/schema/config/lock/fixture seams stay serial); Phase 28 fans in. This map matches `.planning/ROADMAP.md` phase list and requirement IDs (F21-01…F30-05) — overlap, not new scope (see INGEST-CONFLICTS.md INFO).

## Program admission controls (all OPEN)
- source: .planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md §7,§8
- CTRL-01 versioned capability/maturity ledger with pinned Hermes/OpenClaw baselines before Phase 21 (artifact intel/COMPETITIVE-LEDGER.md; families AUTH,TXN,GOAL,CONT,GATEWAY,REACH,PORT,MEDIA,NATIVE,SUPPLY; most rows SOURCE, GATEWAY ABSENT, all peer baselines UNPINNED). CTRL-02/D1 pinned Core producer contract + linked Desktop plan + real consumer/reducer conformance before broad Phase 21 (artifact intel/DESKTOP-PROTOCOL-CHECKPOINT.md). CTRL-03 route packaged/customer evidence into live regression register (artifact intel/FIELD-REGRESSIONS.md). CTRL-04/D2 freeze durable Core producer protocol + replay canonical fixtures through real Desktop consumer/reducer before Phase 23 exits.

## Remaining program spine (to-do)
- source: .planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md §5; MASTER-HANDOFF-phase20-native-parity.md §3
- (1) Finish Phase 20 native path: repair → re-prove → fresh 20-16 (native NOT deferred) → 20-17 → 20-18 (Sean-authorized) → Phase 20 complete. (2) D1/CTRL-02 before broad Phase 21. (3) CTRL-01 pin peer baselines before Phase 21. (4) Phases 21→22→23A→23B (+D2/CTRL-04 at 23 exit) serial. (5) 24,25,26,27 bounded-parallel after Phase 23 (26 = Hermes/OpenClaw migration). (6) Phase 28 fan-in → 29 → 30. (7) CTRL-03 continuous.

## Phase 20 lifecycle build record (accepted, Linux-green)
- source: .planning/inbox/MASTER-HANDOFF-phase20-native-parity.md §2.1
- Plans 20-01…20-16 built + accepted: isolated-checkout substrate (20-03/04), production-spawner routing (20-05, source a528dbc), opaque CandidateSeal (20-06/09), HardContainmentAuthority (20-10/11), gate-execution + durable-receipt AcceptedCandidate (20-12, source b8a260e), black-box proof (20-13, ace4bd2), integrated independent audit (20-14, zero-finding), parent landing/CAS + rollback (20-07, d527ca8), full lifecycle wired into live Anvil climb + native-UAT scaffolding (20-08, source 5e665ec), fresh independent 20-16 review CLEAN with native deferred. This aligns with STATE.md's recorded history.

## Open research questions for the native repair
- source: .planning/inbox/native-uat-repair-BRIEF.md §7; MASTER-HANDOFF-phase20-native-parity.md §4
- (1) Windows Job-Object containment test design (descendant reaping on job close, active-process cap, breakaway denial, exit-code fidelity) — no Windows containment test exists. (2) type_and_hold stdin-free exit-0 delay primitive inside an AppContainer. (3) macOS harness truth — which of 8 targets real vs aspirational. (4) Anti-drift guard enforcement. (5) Whether dropping deny-only SIDs needs adversarial review beyond 20-16.

## Document map + external dossiers (reference)
- source: .planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md §9; MASTER-HANDOFF-phase20-native-parity.md §7
- Charter/roadmap: AGENTS.md, PROJECT.md, REQUIREMENTS.md, ROADMAP.md (authoritative), STATE.md, HANDOFF.json. Design charter: docs/design/2026-07-13-wayland-core-frontier-*.md + f00…f06 contracts. Intel/controls: COMPETITIVE-LEDGER.md, PLAN-V2-ADVISORY.md, FIELD-REGRESSIONS.md, DESKTOP-PROTOCOL-CHECKPOINT.md. External (NOT in repo): support/wcore-research/ + support/forensics/ dossiers — hypotheses/older coordinates, re-locate + verify before treating as proof.
