## Conflict Detection Report

Ingest of 3 inbox docs (2 DOC, 1 SPEC) reconciled against `.planning/STATE.md`,
`.planning/ROADMAP.md`, `.planning/REQUIREMENTS.md`, `.planning/PROJECT.md`.
Mode: merge. No ADRs and no `locked` decisions in the set → no locked-decision
contradictions possible. Cross-ref graph is acyclic. All three classifications
are high-confidence → no UNKNOWN blockers.

### BLOCKERS (0)

None. No LOCKED-vs-LOCKED contradiction, no contradiction of an existing locked
decision, no cycle, no UNKNOWN/low-confidence doc.

### WARNINGS (4)

[WARNING] Plan-count contradiction: inbox 16/18 vs ROADMAP.md progress table 2/18
  Found: All three inbox docs state Phase 20 is 16/18 plans complete (native RED). STATE.md agrees (total_plans 18, completed_plans 16, "Plan 17 of 18", 83%). But .planning/ROADMAP.md progress table (line 171) still reads "20. Transactional Delegated Mutation | 2/18 | Plans 20-01 and 20-02 complete; reopened 20-03 is the next serial execution boundary".
  Impact: ROADMAP.md's own progress table is stale relative to both STATE.md and the inbox; downstream roadmapper could re-plan from 2/18 and duplicate 20-03…20-16.
  → Human: confirm 16/18 is truth and correct the ROADMAP.md progress table (the frontier-v2 doc itself flags the "2/18" transfer as stale-on-state). Do NOT let the roadmapper regenerate 20-01…20-16.

[WARNING] Candidate SHA not recorded in STATE.md
  Found: Inbox docs pin the current candidate as `6937ef6` (tree `6db6fc85`; working commit `be84bd2`, branch `plan/f20-unified-audit-repair`). STATE.md's latest recorded source is 20-08 `5e665ec` (tree `e5dcf77c`) and the 20-16 review; it names no `6937ef6`/`be84bd2` candidate, and its Execution Authority lineage is anchored on `94f014d` / accepted 20-01 `626e1d4d` / 20-02 `96afb30a`.
  Impact: The authoritative candidate SHA the roadmapper/executor should target is ambiguous between STATE.md's recorded chain and the inbox's `6937ef6`/`be84bd2`.
  → Human: reconcile which SHA is the live Phase-20 candidate and record it in STATE.md before native-repair planning; every SHA change also invalidates the prior 20-16 review and the prior 20-18 authorization.

[WARNING] Native status + runner availability contradiction (STATE.md vs inbox)
  Found: STATE.md Blockers/Concerns says "20-17 BLOCKED (2026-07-21): no qualifying self-hosted runner exists … ferrox-win-msvc + SEANDESKTOP (both Windows, both offline); zero macOS runners", native never certified, next plan = 20-17. The inbox docs instead report that 20-18 native UAT was ACTUALLY RUN on real Windows 11 hardware (result RED), that `ferrox-win-msvc`/`SEANDESKTOP` self-hosted msvc runners are ONLINE and AppContainer-capable, and that a macOS 8/8 proof already ran on a Scaleway runner.
  Impact: STATE.md and the inbox disagree on whether native was ever run, whether runners are online, and whether macOS proof exists — directly affects whether the native-repair plan can execute or is still runner-blocked.
  → Human: verify live runner status and the 20-18 RED result, then reconcile STATE.md's 20-17 blocker note against the inbox's "native ran / runners online" claim before scheduling native work.

[WARNING] Next-plan / sequencing contradiction
  Found: STATE.md sets the next boundary as plan 20-17 (then 20-18). The inbox docs re-sequence Phase 20 to insert a native-repair plan (a "repaired 20-08 successor", requirements R1–R12) AND a FRESH 20-16 review (native NOT deferred) BEFORE 20-17/20-18.
  Impact: The immediate next action differs — STATE.md points at 20-17; the inbox points at an unplanned native-repair plan + re-run 20-16. Following STATE.md as-is would skip the native repair.
  → Human: confirm the native-repair-successor + fresh-20-16 sequencing and update STATE.md's "Current Position / next plan" accordingly.

### INFO (6)

[INFO] Precedence applied: SPEC (BRIEF) wins on native-validation reality
  Note: `native-uat-repair-BRIEF.md` is manifest-authoritative SPEC (precedence 0) and self-declares that where it conflicts with prior 20-08/20-16 assumptions about native validation, it wins (those assumptions deferred native and were never tested). Auto-resolved: the "native RED, harness never run" reality supersedes the older "20-08 accepted / native deferred" framing. This is a refinement of prior state, not a new locked decision.

[INFO] Auto-resolved overlap: phase map F20→F30 already exists — no new phases
  Note: The inbox phase map (§3 of the program doc) matches `.planning/ROADMAP.md` phases 20–30 and their requirement IDs (F21-01…F30-05) and execution order (20 → D1 → 21 → 22 → 23A → 23B/D2 → {24–27 parallel} → 28 → 29 → 30). Treated as reconciliation of existing scope; NO duplicate phases or requirements emitted.

[INFO] New additive requirements R1–R12 (native repair) extracted
  Note: The SPEC contributes 12 new, non-overlapping requirements scoped to the Phase-20 native-UAT repair (written to requirements.md). They refine Phase-20 Success Criterion #3 (native Windows/macOS lifecycle) and do not duplicate F20-01…F20-06 / F20-GATE-01/02. Two of the underlying fixes (4.A storage `.write(true)`, 4.B drop deny-only SIDs) are proven-on-hardware but uncommitted spikes to be re-derived under the plan.

[INFO] Program controls CTRL-01…04 / D1 / D2 remain OPEN
  Note: Consistent across inbox and STATE.md/ROADMAP.md — competitive ledger unpinned (Hermes/OpenClaw baselines UNPINNED), D1 required before broad Phase 21, D2 by Phase 23 exit. No conflict; recorded for the roadmapper.

[INFO] Prior 20-18 authorization spent; new candidate needs fresh Sean authorization
  Note: Inbox states the prior 20-18 authorization (`cb6f06bd…`) is spent/void; a RED result or any new candidate SHA requires fresh Sean authorization. Aligns with STATE.md's Sean-only gate policy. No action for synthesis; flagged for the executor.

[INFO] Hygiene: tainted local tree + stale Cargo.lock
  Note: Inbox reports the local `waylandcore-ferrox` working tree has diagnostic edits in `windows_impl/process.rs` and `storage.rs` to reset to pristine `be84bd2`, and Cargo.lock is stale at `be84bd2`. Captured as constraints (constraints.md) and requirement REQ-native-r10; not a synthesis conflict.
