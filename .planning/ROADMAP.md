# Roadmap: Wayland Core Frontier Candidate v2

## Overview

This roadmap resumes the accepted Frontier program at F20, preserves F00-F19 as historical evidence, and advances one admitted candidate through transactional agency, governed continuous agency, operator-complete product surfaces, native certification, supply-chain proof, and independent peer review. Later phases may be planned; execution is limited to the dependency-unlocked phase or to the explicitly declared bounded Phase 24-27 workstreams after Phase 23 admission.

## Execution Rules

- Standard GSD is the phase-state and execution authority. One clean standalone source clone whose `.git` is a directory contains the code, plans, and standard summaries; linked worktrees are forbidden for Phase 20 execution because stock GSD otherwise suppresses normal STATE/ROADMAP updates. GitHub only mirrors cross-lane coordination.
- Exactly one integration candidate advances through one serial execution sequence from the reconciled source `94f014d039b8babf3f5926385a3bbc5cb5cf3c41` (tree `49635e1678bd96e42353ab0f7f943ba87497e9d0`); it already contains the accepted 20-01 and 20-02 source lineages, their standard summaries, and post-summary repairs. The reopened 20-03 begins only from the independently accepted planning successor of that source; `depends_on` expresses only genuine product dependencies.
- Phase 20 WIP is one dependency-unlocked product plan. Read-only review may overlap, but product mutation remains serial.
- Phases 20-23 execute serially. After Phase 23 admission, Phases 24-27 may execute in bounded parallel worktrees; shared protocol/schema/config/lock/fixture seams and final integration remain serial. Phase 28 fans them in.
- `intel/COMPETITIVE-LEDGER.md`, `intel/FIELD-REGRESSIONS.md`, and `intel/DESKTOP-PROTOCOL-CHECKPOINT.md` are admission controls. D1 must pass before Phase 21 broad execution; D2 must pass by Phase 23 exit.
- Source push, main merge, issue closure, release, deployment, canary promotion, native proof dispatch, and deletion of a retained candidate UAT evidence ref require Sean's explicit authorization.
- Phase 20 source plans deliver product code or hostile tests plus focused proof and one standard SUMMARY. Fresh non-author reviews use separate dependency-ordered stock-GSD plan boundaries so author and reviewer identity cannot collapse inside one executor; repository-local scope/review verifiers mechanically reject stale or self-referential evidence.
- GSD node repair, worktree mode, and Phase 20 plan parallelization are disabled (`workflow.use_worktrees=false`). The Codex adapter is one clean serial standalone clone with a real `.git` directory, not a linked worktree or custom executor.
- Phase auto-advance is ENABLED (`workflow.auto_advance=true`), reversing the earlier prohibition. Manual gating between phases was one half of the stall that grew Phase 20 to 74 plans; the other half was `workflow.security_block_on=low`, which let a LOW finding block advancement. Both are now bounded: advancement blocks only on CRITICAL or HIGH (`workflow.security_block_on=high`). Auto-advance moves between phases; it does not authorize a push, merge, release, issue closure, or native-proof dispatch — those remain Sean-gated above.
- Preparation plan 20-17 reauthenticates the exact 20-16-qualified source and persists one idempotent pending native-proof tuple without external mutation. Terminal plan 20-18 alone accepts Sean's exact-tuple authorization, runs candidate-specific native Windows/macOS acceptance, and runs the configured remote build/workspace test once before requirement completion; the phase verifier authenticates rather than replacing those gates. Timeout, nonzero, incomplete output, unresolved CRITICAL or HIGH review findings, failed validation/security, or missing authorized same-candidate native evidence blocks phase completion.
- Plan 20-16 performs blocking independent code review, phase validation, and ASVS Level 2 security prosecution against the exact construction source. Findings at CRITICAL or HIGH must be fixed or disproved before native UAT and `phase.complete`; MEDIUM and below are logged to BACKLOG and do not block execution. Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is not permitted; it escalates to Sean. Advisory post-phase checks cannot override this dependency gate.

## Phases

- [ ] **Phase 20: Transactional Delegated Mutation** - Integrate one fail-closed workspace, journal, gate, receipt, merge, rollback, and cleanup lifecycle.
- [x] **Phase 20A: Native Windows/macOS UAT Repair** - Prove the admitted Phase-20 candidate's delegated-mutation lifecycle natively on Windows and macOS. **COMPLETE 2026-07-26** @ `9821ef76`, run `30184651330`. (This row was absent from the checklist; added at closeout so the phase is visible here as well as in Phase Details.)
- [ ] **Phase 21: Child Authority and Budget Inheritance** - Prove that delegation cannot amplify authority or resources.
- [ ] **Phase 22: Supervision, Durable Goals, Fleet, and Loops** - Expose one durable, restart-safe Goal/Task/Wait and child-supervision contract.
- [ ] **Phase 23: Governed Continuous Personal Agency** - Complete safe learning, operator recovery, memory controls, waits, and context economics.
- [ ] **Phase 24: Gateway, Automation, Channels, and Typed API** - Deliver the persistent runtime and daily operator/control-plane lifecycle.
- [ ] **Phase 25: Remote Reach, Nodes, and Plugin Lifecycle** - Prove governed execution and ecosystem behavior across bounded reference backends.
- [ ] **Phase 26: Migration, Export, Backup, and Restore** - Make state portable, quarantined, reversible, and secret-safe.
- [ ] **Phase 27: Multimodal, Browser, Generation, and Voice Contracts** - Make media and interactive capabilities reliable and honestly capability-gated.
- [ ] **Phase 28: Native Cross-Platform Certification** - Prove the same hostile, reliability, and soak contract on macOS, Linux, and Windows.
- [ ] **Phase 29: Supply Chain and Release Integrity** - Bind trusted source and evidence to installable, updateable, revocable artifacts.
- [ ] **Phase 30: Continuous Scorecard and Frontier Review** - Independently decide which frontier claims the evidence supports.

## Phase Details

### Phase 20: Transactional Delegated Mutation
**Goal**: Delegated coding can mutate only an isolated workspace and can affect the parent only through a gated, attributable, reversible integration.
**Depends on**: None
**Accepted Source**: F00-F19 entered Phase 20 at `97e44910fc6dd4761f1f862dbf54a5a76262cef2` (tree `8d27bd96b476a728d3ebbe0e1583c6488dd5effc`). The reconciled source before reopened 20-03 execution is `94f014d039b8babf3f5926385a3bbc5cb5cf3c41` (tree `49635e1678bd96e42353ab0f7f943ba87497e9d0`). It contains accepted 20-01 source `626e1d4d3dee9fee7008ad172ec0b4add8f2004e`, accepted 20-02 source `96afb30aff362ef8f0d4f6f93773eae548d989ee`, both standard summaries, and the later source repairs recorded in their history. The independently accepted plan successor of `94f014d` is captured as `F20_03_EXECUTION_BASE` immediately before Task 1A and is the sole source checkout for all remaining Phase 20 work.
**Requirements**: F20-01, F20-02, F20-03, F20-04, F20-05, F20-06, F20-GATE-01, F20-GATE-02
**Scope fence**: The additive native-UAT repair requirements `REQ-native-r1…r15` are NOT Phase 20. They moved to Phase 20A. Phase 20 is done when its four Success Criteria are TRUE — not when a plan list is empty.
**Success Criteria** (what must be TRUE):
  1. Parallel children can make conflicting edits without overwriting or silently mutating the parent.
  2. Stale identity, failed gates, and conflicts stop before merge while preserving usable evidence.
  3. Snapshot, workspace, journal, receipt, merge, rollback, cleanup, and native Windows/macOS identities share one authoritative lifecycle.
  4. One exact F20 successor lands on the admitted candidate with focused and aggregate proof.
**Plans**: Eighteen dependency-ordered stock-GSD plans (`20-01` through `20-18`). Foundational isolation source plan `20-03` is independently reviewed by review-only plan `20-15` before caller wiring begins in `20-04`. The original candidate-gate plan is split into 06A source/review (`20-06`, `20-09`), 06B source/review (`20-10`, `20-11`), 06C private authoritative execution (`20-12`), 06D black-box proof (`20-13`), and an exact-candidate independent integrated audit (`20-14`). Caller integration (`20-05`) resumes only after `20-14`; parent CAS (`20-07`) follows `20-05`. Plan `20-08` constructs and focused-proves the final lifecycle/UAT machinery; review-only `20-16` prosecutes that exact source at every severity; autonomous plan `20-17` durably prepares one idempotent pending request without external mutation; terminal plan `20-56` alone owns requirements and runs the explicitly authorized candidate-specific aggregate acceptance. The later stock-GSD phase verifier authenticates that exact-candidate proof.

**Live plan set (post-reconciliation, 2026-07-25)**: eighteen plans — `20-01`…`20-17` plus terminal `20-56`. Every one traces to a Success Criterion; see `RECONCILIATION.md`. Fifty-six plans were archived, not deleted: `archive/native-split/` (51, moved to Phase 20A) and `archive/superseded/` (5 — `20-18`, `20-28`, `20-35`, `20-42`, `20-49`, successive generations of the same terminal-authorization step, of which `20-56` is the live head).

### Phase 20A: Native Windows/macOS UAT Repair — **COMPLETE (2026-07-26)**
**Goal**: The admitted Phase-20 candidate proves its delegated-mutation lifecycle natively on Windows and macOS.
**Depends on**: Phase 20
**Status**: **COMPLETE.** Closed on 2026-07-26 at sealed SHA `9821ef7603ac1e687b600cda591af1657c883484` (tree `0a1267a990f3b512782916b6ed26501d0db39222`, tag `f20a-candidate-9821ef76`, local `refs/f20a/candidate`), proved by ONE newly dispatched Sean-authorized run — `nightly-windows-soak` **run `30184651330`** (`workflow_dispatch`, `--ref f20a-candidate-9821ef76`, `completed`/`success`, 2026-07-26T02:30:03Z→02:48:08Z). `F20_NATIVE_WINDOWS_ACCEPTANCE=PASS` (job `89747993276`, runner `ferrox-win-msvc`/SEANDESKTOP, **6/6** targets) and `F20_NATIVE_MACOS_ACCEPTANCE=PASS` (job `89747992986`, runner `f20-macos-ephemeral-1d053640`, **8/8** targets), both at the same commit/tree/nonce `96c91107…`. Zero red targets. Both candidate jobs asserted the authorized SHA against the real checkout before any build step. Evidence: `phases/20A-native-windows-macos-uat/20A-04-SUMMARY.md` §13; phase state: that directory's `.continue-here.md`.
**Requirements**: REQ-native-r1…r12, plus audit addenda r13–r15 (full text in `.planning/intel/inbox-2026-07-23/requirements.md`; addenda in `AUDIT-2026-07-23.md`). **At close: 11 complete** (r1, r3, r4, r5, r6, r7, r9, r10, r11, r14, r15) **and 4 explicitly OPEN** (r2, r8, r12, r13) — each an unexercised acceptance clause rather than a failing test, and none of them a Success Criterion. Per-requirement evidence is in `REQUIREMENTS.md`.
**Success Criteria** (what must be TRUE):
  1. [x] The six-target native Windows proof passes on the certified self-hosted runner against one exact sealed candidate. — 6/6 PASS, `ferrox-win-msvc`, markers bound to `9821ef76`/`0a1267a9`/`96c91107`.
  2. [x] The macOS leg passes against that same exact candidate. — 8/8 PASS, identical commit/tree/nonce triple.
  3. [x] Native evidence is bound to one newly dispatched, Sean-authorized run — never inferred from source, cross-compilation, Linux proof, or a reused run. — Run `30184651330`, created 2026-07-26T02:30:03Z, fired once on Sean's explicit instruction; the preceding soak run was `30149496548` (scheduled, headSha `61b79c4f`), so nothing was reused.
**Plans**: 4 plans — a HARD CAP, set because Phase 20 metastasized to 74 plans when two finding rules could not terminate (bounded by `d0837aa7`). Fifty-one prior native plans are retained in `phases/20-transactional-delegated-mutation/archive/native-split/` as history, NOT as a live queue. They are seven repeating generations (fix → re-seal → cross-audit → dispatch → post-native review → re-prep → authorize) of the same steps re-planned against successive candidate SHAs; see `RECONCILIATION.md`. This phase was replanned from its Success Criteria, not by resuming that chain.

**Plans (4 live, 4 completed — the hard cap held):**
- [x] 20A-01-PLAN.md — Close the CI blind spot and establish the true native baseline: move the Windows box to the phase SHA, settle whether the 155 Windows-only and 23 macOS-only tests COMPILE, wire `wcore-sandbox` and the ten orphaned `live_fs_acl` ACL security tests into runners that actually execute, give CI a trigger that fires on this branch, and re-measure the four Windows suites with every failure named. (wave 1) — **Outcome: terminated COMPILE-BLOCKED, then cleared.** Baseline established; affirmative per-crate GREEN compile verdict for `wcore-sandbox` (105 of the 155 Windows-only tests) and `wcore-agent`; all twelve `live_fs_acl` names confirmed present and wired to a non-proof runner; the proof script left byte-identical. No requirement completed.
- [x] 20A-02-PLAN.md — Close the one structural blocker: bind the AppContainer child's working directory to the retained delegated workspace authority without weakening the anti-swap guarantee. One hardware-measured mechanism evaluation, one blocking decision, one implementation. Unblocks 7 swarm + 4 agent + 4 dispatch_smoke failures on one cause. (wave 2) — **Outcome: BIND LANDED** (termination state 1, shipped at `c252d01d`). Child cwd now bound to the retained workspace authority under an OS-enforced name pin, via a scoped process-lifetime lease; no existing open changed share mode, no existing test changed.
- [x] 20A-03-PLAN.md — Force an explicit decision on how the landing's deterministic-checkout security scrub reconciles with Windows end-of-line semantics, after first determining single-variable what the ACTUAL mechanism is — the reported one is contradicted by the committed `.gitattributes`. (wave 2) — **Outcome: REFUTED-NO-DEFECT** (termination state 2). The dirty-checkout symptom is a TEST FIXTURE artifact, not a product defect; production mints the integration checkout through the same scrub that later judges it. The in-tree `.gitattributes` `eol=lf` rule was measured to override `core.autocrlf` and survive `GIT_ATTR_NOSYSTEM=1` — the brief's mechanism vindicated, its conclusion refuted. No production file changed.
- [x] 20A-04-PLAN.md — Seal ONE exact candidate, prosecute it with schema-validated per-reviewer review artifacts (r13), and PREPARE the Sean-gated six-Windows-plus-eight-macOS native proof dispatch without firing it (PRD D6). (wave 3) — **Outcome: DISPATCHED GREEN** (effective termination state 1; the plan itself first terminated in state 4 at the stale seal `50cf00b3` with 4 GREEN / 2 RED, which is retained in `20A-04-SUMMARY.md` §1–§11 as history). Re-sealed at `9821ef76`, re-proved 6/6 locally, then fired run `30184651330` on Sean's authorization: Windows 6/6 + macOS 8/8. Note that r13's own deliverable — per-reviewer schema-validated review artifacts — was **not** produced, which is why r13 is recorded OPEN.

**Termination discipline (every plan carries it verbatim)**: findings at CRITICAL or HIGH must be fixed or disproved; MEDIUM and below are logged to BACKLOG and DO NOT BLOCK execution. Execution begins when no CRITICAL or HIGH finding is open, or after 2 review rounds, whichever comes first. A third round is NOT permitted; it escalates to Sean. Each plan additionally names the exact conditions under which it STOPS and escalates rather than spawning more work.

**Origin**: the `20-18` native UAT ran RED against the pre-repair candidate. The repair work that followed was additive to Phase 20 and never belonged to its Success Criteria; it is separated here so Phase 20 can close on what it actually promised.

### Phase 21: Child Authority and Budget Inheritance
**Goal**: Every delegated actor remains inside the parent's authority and resource envelope.
**Depends on**: Phase 20
**Requirements**: F21-01, F21-02, F21-03, F21-04
**Success Criteria** (what must be TRUE):
  1. A child cannot widen any provider, tool, filesystem, egress, secret, approval, depth, fan-out, time, token, or cost restriction.
  2. Nested reservation, refund, escalation, approval, cancellation, and result delivery remain attributable to the correct parent/session.
  3. Standalone and host-protocol hostile corpora prove equivalent enforcement.
**Plans**: TBD
**Admission prerequisite**: before broad Phase 21 execution — (CTRL-02 / D1) the linked Desktop plan and consumer/reducer conformance harness are pinned to the exact Core producer contract, AND (CTRL-01) the Competitive Capability Ledger has pinned exact Hermes + OpenClaw baselines with F03/F05 evidence mapped.

### Phase 22: Supervision, Durable Goals, Fleet, and Loops
**Goal**: Users can supervise durable objectives and work graphs through one restart-safe lifecycle and one loop owner.
**Depends on**: Phase 21
**Requirements**: F22-01, F22-02, F22-03, F22-04, F22-05, F22-06, F22-07
**Success Criteria** (what must be TRUE):
  1. CLI, TUI, and host-protocol paths observe and control identical Goal, child, task, wait, log, cursor, and terminal producer state, and emit the canonical serialized producer fixtures consumed later at D2.
  2. Fleet claims and dependencies survive kill/restart/reassignment without duplicate execution or lost completion.
  3. Direct, ForgeFlows, Fleet, Council, and Anvil terminate through one canonical Goal transition with no nested verification/retry owner.
  4. Session-local fixed/dynamic, event-driven, and manual loops remain bounded across reconnect, preemption, missed intervals, and resume; persistent scheduling is deferred explicitly to Phase 24.
  5. Existing journal compatibility is proved or migrated explicitly without silently invalidating F12 behavior.
**Plans**: TBD

### Phase 23: Governed Continuous Personal Agency
**Goal**: The agent can pursue verified outcomes over time, learn safely, and let users inspect, correct, recover, and control that state.
**Depends on**: Phase 22
**Requirements**: F23-01, F23-02, F23-03, F23-04, F23-05, F23-06
**Success Criteria** (what must be TRUE):
  1. Generated skills cannot execute before governed promotion and can be observed, revoked, and rolled back.
  2. Users can search, inspect, checkpoint, retry, fork, rewind, export, retain, and reconcile session effects.
  3. Users can see and control memory/user-model activation, provenance, correction, forgetting, privacy, retention, and nudges.
  4. Cache and compaction behavior expose quality, invalidation, token-pressure, and cost truth.
  5. A multi-day wait/resume/complete journey preserves cumulative authority, resource, evidence, memory, and delivery state.
  6. A persistent incremental hybrid repository index provides bounded lexical/symbol/optional-semantic retrieval with provenance, staleness, privacy and performance truth.
**Plans**: TBD
**Internal order**: Phase 23A closes governed-skill/M3 contracts first. Phase 23B Continuous Agency (operator lifecycle, memory, index, cache economics, multi-day journey) begins only from the admitted 23A contract.
**Exit gate**: D2 freezes durable child/Goal/task/wait commands, events, cursors and delivery semantics and passes real Desktop consumer/reducer replay.

### Phase 24: Gateway, Automation, Channels, and Typed API
**Goal**: Operators can install, run, automate, connect, inspect, recover, and support one persistent Core runtime on every OS family.
**Depends on**: Phase 23
**Requirements**: F24-01, F24-02, F24-03, F24-04, F24-05
**Success Criteria** (what must be TRUE):
  1. Native service lifecycle, profile isolation, active-turn visibility, drain, restart, upgrade, and rollback work without lost or duplicate delivery.
  2. Scheduled, event-driven, webhook, polling, and commitment work has bounded history, retry, continuation, and delivery.
  3. Reference channels prove setup/auth, access, routing, media, native actions, idempotency, reconnect/reload, and health.
  4. Typed authenticated clients recover event gaps and produce useful redacted health/log/support evidence.
  5. Setup-to-recovery journeys pass on macOS, Linux, and Windows.
**Plans**: TBD

### Phase 25: Remote Reach, Nodes, and Plugin Lifecycle
**Goal**: Operators can run governed work across reference backends/nodes and manage plugins through a complete, recoverable lifecycle.
**Depends on**: Phase 23
**Requirements**: F25-01, F25-02, F25-03, F25-04, F25-05
**Success Criteria** (what must be TRUE):
  1. The same task runs locally, in a container, over SSH, and on one hibernating cloud backend with equivalent policy, receipts, cancellation, and cleanup.
  2. Nodes pair, advertise capability, revoke, recover offline, and handle mixed versions without losing authority attribution.
  3. Plugins can be scaffolded, tested, signed, installed, approved, inspected, updated, rolled back, removed, published, and recovered.
  4. Compromised keys/plugins/backends and denied secret/egress paths fail closed with no orphaned execution.
**Plans**: TBD

### Phase 26: Migration, Export, Backup, and Restore
**Goal**: Users can move, preserve, and restore Wayland state without executing imported content or leaking secrets.
**Depends on**: Phase 23
**Requirements**: F26-01, F26-02, F26-03, F26-04, F26-05
**Success Criteria** (what must be TRUE):
  1. Hermes/OpenClaw discovery and dry-run are typed, deterministic, secret-redacted, and non-mutating.
  2. Selective import/export preserves provenance and quarantines executable content.
  3. Backup, restore, profile migration, and reciprocal portability survive interruption and restore exact pre-operation state on rollback.
  4. Hostile fixture corpora prove conflict, secret-source remapping, isolation, and recovery semantics.
**Plans**: TBD

### Phase 27: Multimodal, Browser, Generation, and Voice Contracts
**Goal**: Attachments, documents, browser/CUA/web, generation, and voice work consistently across providers and hosts with honest readiness and bounded authority.
**Depends on**: Phase 23
**Requirements**: F27-01, F27-02, F27-03, F27-04, F27-05
**Success Criteria** (what must be TRUE):
  1. Standalone and host messages use one bounded, validated attachment/document intake path and degrade explicitly on unsupported providers.
  2. Browser, CUA, and web surfaces publish live readiness and preserve sandbox, egress, approval, and cleanup policy.
  3. Built-in, MCP-only, late-MCP, and combined media generation expose consistent discovery, credentials, accounting, and failures.
  4. Streaming voice supports interruption, cancellation, compatibility, accounting, and ordered protocol events.
  5. Deterministic corpora and packaged smokes pass on native macOS, Linux, and Windows.
**Plans**: TBD

### Phase 28: Native Cross-Platform Certification
**Goal**: The exact candidate proves its security, reliability, recovery, and resource contract natively on all required OS families.
**Depends on**: Phases 24, 25, 26, and 27
**Requirements**: F28-01, F28-02, F28-03, F28-04
**Success Criteria** (what must be TRUE):
  1. Native macOS, Linux, and Windows pass the required hostile platform matrix with no skipped critical case.
  2. The 1,000-session/concurrent-child soak has no secret leak, orphan process, unbounded resource use, or unacceptable quality/performance delta.
  3. Signed receipts bind exact candidate, platform, posture, corpus, environment, artifacts, logs, and skip policy.
  4. Zero findings remain at every severity before acceptance.
**Plans**: TBD

### Phase 29: Supply Chain and Release Integrity
**Goal**: Users can verify that installed and updated artifacts correspond to the source, evidence, policy, and trust roots that earned acceptance.
**Depends on**: Phase 28
**Requirements**: F29-01, F29-02, F29-03, F29-04
**Success Criteria** (what must be TRUE):
  1. Clean-room builds verify provenance, SBOM, dependency policy, signatures, and reproducibility or documented variance.
  2. Install and update paths verify source/artifact identity, rollback/freeze protection, revocation, and key rotation.
  3. Tampered artifacts, manifests, receipts, plugins, backends, or keys are rejected.
  4. Packaging, deployment preparation, rollback rehearsal, and release acceptance remain separate evidence and authorization states.
**Plans**: TBD

### Phase 30: Continuous Scorecard and Frontier Review
**Goal**: Sean can make a defensible release-positioning decision from repeated, comparable, independently reviewed evidence.
**Depends on**: Phase 29
**Requirements**: F30-01, F30-02, F30-03, F30-04, F30-05
**Success Criteria** (what must be TRUE):
  1. Every surface has versioned activation, operator-completeness, maturity, security-owner, evidence, and peer-delta truth refreshed at each phase.
  2. Wayland, Hermes, and OpenClaw complete common correctness, recovery, security, cost, and cognitive-tax trials with pinned baselines and confidence bounds.
  3. Published claims and limitations match raw redacted evidence and contain no unsupported superiority language.
  4. No main merge, issue closure, release, deployment, or frontier positioning occurs without Sean's explicit approval.
**Plans**: TBD

## Progress

**Execution Order:** Phase 20 → D1 → 21 → 22 → 23A → 23B/D2 → {24, 25, 26, 27 bounded parallel} → 28 → 29 → 30

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 20. Transactional Delegated Mutation | 16/28 | 20-01…20-16 complete. Candidate `6937ef6` (tree `6db6fc85`; working commit `be84bd2`); Linux `nextest --profile ci` 11509/0 (Hetzner); 20-16 review CLEAN (native deferred by design). **Native path RED/incomplete** — recorded 20-18 ran RED 2026-07-22 vs PRE-repair `5e665ec` (Windows compile + macOS test-mapping failures); a 2026-07-23 off-plan diagnostic found a deeper AppContainer read/token issue; Windows+macOS native UNPROVEN for `6937ef6`. Ten additive native-repair successor plans **planned** (`20-19`…`20-28`, REQ-native-r1…r15): reset+sandbox fixes (19) → test bugs + isolation proofs (20) → Windows Job-Object tests + anti-drift guard (21) → macOS re-validate + self-hosted runner + review profile (22) → seal + Cargo.lock + Hetzner aggregate (23) → cross-audit (24) → Sean-gated native-proof dispatch (25) → fresh review native-NOT-deferred (26) → re-prep (27) → Sean-authorized terminal (28, completes F20). Ready to execute `/ferrox-execute-phase 20` from wave 18. | - |
| 21. Child Authority and Budget Inheritance | 0/TBD | Not started | - |
| 22. Supervision, Durable Goals, Fleet, and Loops | 0/TBD | Not started | - |
| 23. Governed Continuous Personal Agency | 0/TBD | Not started | - |
| 24. Gateway, Automation, Channels, and Typed API | 0/TBD | Not started | - |
| 25. Remote Reach, Nodes, and Plugin Lifecycle | 0/TBD | Not started | - |
| 26. Migration, Export, Backup, and Restore | 0/TBD | Not started | - |
| 27. Multimodal, Browser, Generation, and Voice Contracts | 0/TBD | Not started | - |
| 28. Native Cross-Platform Certification | 0/TBD | Not started | - |
| 29. Supply Chain and Release Integrity | 0/TBD | Not started | - |
| 30. Continuous Scorecard and Frontier Review | 0/TBD | Not started | - |
