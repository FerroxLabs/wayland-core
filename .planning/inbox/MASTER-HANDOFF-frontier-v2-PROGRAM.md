# MASTER HANDOFF — Wayland Core Frontier Candidate v2 (WHOLE PROGRAM, F00→F30)

> **This is the single authoritative consolidation of the entire program** — the parity effort (vs Hermes /
> OpenClaw), the transactional/agency refactor, and the path to a shippable, independently-reviewed release.
> It is a **Ferrox Factory source document**: ingest it (`/ferrox-ingest-docs` over `.planning/inbox/`),
> reconcile against live `.planning/`, and drive the program with discipline.
>
> **Supersedes (updates):** `.planning/exports/2026-07-20-CLAUDE-FULL-PROGRAM-TRANSFER.md` — that transfer is
> correct on the roadmap but **stale on state** (it says "2/18 plans"; reality is 16/18 + a native repair).
> Use THIS doc for current state; use that transfer + the `docs/design/2026-07-13-*` set for the deep charter.
>
> **Owner:** core lane (area:core). **Date:** 2026-07-23. **Program:** Frontier Candidate v2 (F00–F30).
> **Children of this doc:** `.planning/inbox/MASTER-HANDOFF-phase20-native-parity.md` (Phase-20 detail map)
> and `.planning/inbox/native-uat-repair-BRIEF.md` (the current-blocker repair spec).

---

## 0. Why this document exists (read first)

The program lost focus: execution tunneled into Phase 20's Windows sandbox and drifted into ad-hoc,
un-gated hacking, instead of running under Ferrox/GSD discipline with the whole F00→F30 program in view.
This doc pulls everything back into one place so the program can proceed phase-by-phase with build discipline.
**The plan already exists and is good.** The problem was execution discipline, not the plan.

---

## 1. Program objective + the parity thesis (what "done" means)

**Deliver a best-in-class cross-platform agent** that stays simple in ordinary use, is bounded/auditable under
policy, crash-complete, transactionally delegated, operator-complete, and **honestly proven through the
packaged product** — achieving **functional parity or advantage against pinned Hermes and OpenClaw baselines.**

**Positioning thesis (from PROJECT.md):** *"Hermes-grade persistent agency and OpenClaw-grade product
completeness on Wayland's stronger authority, recovery, transaction, and evidence spine."*
- **Wayland already leads** on: authority, containment, recovery, transactional delegation, provider-neutral
  orchestration, evidence design.
- **Hermes leads** on: persistent completion, session/operator lifecycle, proactive automation, execution reach.
- **OpenClaw leads** on: gateway/channel/app/plugin product completeness.
- **Parity is a program-level, evidence-gated goal** — tracked by the Competitive Capability Ledger (CTRL-01),
  the Hermes/OpenClaw migration surface (Phase 26), and the final Wayland/Hermes/OpenClaw trials (Phase 30).
  **Source presence never earns parity; only F28–F30 packaged/native/peer evidence does.**

**Acceptance for the whole program:** one exact candidate satisfies **all 58 active requirements + 4 controls**
with zero unresolved findings at every severity, required native-platform evidence, packaged standalone/host
journeys, supply-chain proof, and independently-reviewed peer positioning.

---

## 2. Where we ACTUALLY are (one-glance state, 2026-07-23)

| Scope | Status |
|---|---|
| **F00–F19** | Accepted historical baseline (preserved, not re-executed). Entered F20 at `97e4491` (tree `8d27bd96`). |
| **Phase 20 (Transactional Delegated Mutation)** | **16/18 plans complete.** Candidate `6937ef6` (tree `6db6fc85`; working commit `be84bd2`). Linux-green (`nextest --profile ci` 11509/0), macOS 8/8, accepted through fresh independent 20-16 review — **but 20-16 DEFERRED native**. **20-18 native UAT is RED** (Windows). Native repair discovered + core fixes proven, **not yet planned/committed**. 20-17 + 20-18 pending. |
| **Phases 21–30** | **Not started.** Requirements, goals, success criteria, admission gates, and sequencing exist; **detailed GSD PLAN files are `TBD`** (authored at each admission boundary). |
| **Program controls CTRL-01…04 / D1 / D2** | **Open.** Competitive ledger unpinned (Hermes/OpenClaw baselines `UNPINNED`); D1/D2 Desktop-protocol checkpoints pending. |
| **Sean-only gates** | No push/main-merge/release/deploy/issue-close/native-dispatch/UAT-ref-delete performed. |

**Two-weeks-on-Phase-20 root cause:** the 20-08 native-proof machinery (Windows AppContainer sandbox +
Windows/macOS proof harness) was **authored without ever running on the target OS** ("aspirational native
machinery"), so every native validation attempt surfaces pre-existing breakage. It is now fully mapped and
bounded (see §6 + the BRIEF), but must be finished under a plan, not by improvising.

---

## 3. The full phase map (F20→F30)

Execution order: **Phase 20 → D1 → 21 → 22 → 23A → 23B/D2 → {24, 25, 26, 27 bounded-parallel} → 28 → 29 → 30.**
20–23 serial; only after Phase 23 admission may 24–27 use bounded parallel worktrees (shared
protocol/schema/config/lock/fixture seams stay serial); Phase 28 fans them in.

| Phase | Outcome | Reqs | Depends | Plans | Status |
|---|---|---:|---|---|---|
| **20** | Transactional Delegated Mutation | 8 | F00–F19 | 18 plans | **16/18; native RED** |
| **21** | Child Authority & Budget Inheritance | 4 | 20 + D1 | TBD | Not started |
| **22** | Supervision, Durable Goals, Fleet, Loops | 7 | 21 | TBD | Not started |
| **23** | Governed Continuous Personal Agency (23A→23B, D2 at exit) | 6 | 22 | TBD | Not started |
| **24** | Gateway, Automation, Channels, Typed API | 5 | 23 | TBD | Not started |
| **25** | Remote Reach, Nodes, Plugin Lifecycle | 5 | 23 | TBD | Not started |
| **26** | Migration, Export, Backup, Restore (**Hermes/OpenClaw import**) | 5 | 23 | TBD | Not started |
| **27** | Multimodal, Browser, Generation, Voice | 5 | 23 | TBD | Not started |
| **28** | Native Cross-Platform Certification (macOS/Linux/Windows + 1,000-session soak) | 4 | 24–27 fan-in | TBD | Not started |
| **29** | Supply Chain & Release Integrity | 4 | 28 | TBD | Not started |
| **30** | Continuous Scorecard & Frontier Review (**Wayland vs Hermes vs OpenClaw trials**) | 5 | 29 | TBD | Not started |

Per-phase goals + success criteria: `.planning/ROADMAP.md` (authoritative) and the 2026-07-20 transfer §"Phase
outcomes". Do **not** fabricate 21–30 plan detail before each admission boundary.

---

## 4. EVERYTHING DONE (validated)

### 4.1 F00–F19 — accepted baseline
Characterization + eval/receipt/fixture/capability/containment contracts (`docs/design/2026-07-13-wayland-core-f00…f06-*.md`). Preserved as historical evidence; not re-run.

### 4.2 Phase 20 lifecycle (plans 20-01 → 20-16) — built, Linux-green, reviewed
The whole transactional delegated-mutation lifecycle is built and accepted (detail per
`.planning/phases/20-transactional-delegated-mutation/20-XX-{PLAN,SUMMARY,INDEPENDENT-REVIEW,AUDIT}.md` and
the Phase-20 master handoff). Highlights: isolated-checkout substrate (20-03/04), production-spawner routing
(20-05), opaque CandidateSeal (20-06/09), HardContainmentAuthority (20-10/11), gate-execution + durable-
receipt AcceptedCandidate (20-12), black-box proof (20-13), integrated independent audit (20-14), parent
landing/CAS + rollback (20-07), full lifecycle wired into live Anvil climb + native-UAT scaffolding (20-08),
fresh independent review CLEAN with native deferred (20-16). **Candidate `6937ef6`: Linux 11509/0, macOS 8/8.**

### 4.3 Phase 20 native investigation (2026-07-23) — discovery + proven core fixes
Ran 20-18 on real Windows 11 hardware; root-caused the native breakage; **the sandbox security boundary is
proven correct** after two fixes (both spike-proven, uncommitted). Full detail: **`native-uat-repair-BRIEF.md`**.
Also: killed a stray forked-session copy that was editing files concurrently (process hygiene, not a code issue).

---

## 5. EVERYTHING TO-DO (the remaining program spine)

1. **Finish Phase 20 native path** (the current blocker — see §6 + BRIEF): repair → re-prove → fresh 20-16
   (native NOT deferred) → 20-17 → 20-18 (Sean-authorized) → **Phase 20 complete.**
2. **D1 / CTRL-02** — publish pinned Core producer contract + linked Desktop plan + real Desktop consumer/
   reducer conformance suite **before broad Phase 21 execution**.
3. **CTRL-01** — pin exact Hermes + OpenClaw versions; bootstrap + map F03/F05 evidence into the ledger
   **before Phase 21**.
4. **Phases 21 → 22 → 23A → 23B (+ D2 / CTRL-04 at 23 exit)** — serial; author + audit each GSD plan set at its
   admission boundary.
5. **Phases 24, 25, 26, 27** — bounded-parallel after Phase 23 admission (26 includes Hermes/OpenClaw migration).
6. **Phase 28 (fan-in) → 29 → 30** — native certification + 1,000-session soak → supply-chain/release integrity
   → continuous scorecard + independent Wayland/Hermes/OpenClaw frontier review.
7. **CTRL-03 (FIELD-REGRESSIONS)** — keep routing contradictory packaged/customer evidence throughout.

---

## 6. THE CURRENT BLOCKER — Phase 20 native repair (summary; full spec in the BRIEF)

The 20-18 native path can't pass until the native machinery is repaired. Verified defect classes:
- **Sandbox (real, fix PROVEN, uncommitted):** `is_available()` false everywhere → `storage.rs` OpenOptions
  missing `.write(true)`; sandboxed processes could read **no file** → `CreateRestrictedToken` deny-only SIDs
  break the AppContainer grant path (dropping them restores reads AND preserves isolation).
- **Real compile bug:** `wcore-agent` imports `READ_CONTROL`/`WRITE_DAC` from the wrong `windows-sys` module.
- **Test bugs:** `type_and_hold` asserts on `choice.exe`'s exit index (never 0); `dispatch_smoke` uses a
  non-portable `fs::rename` of an open dir.
- **Harness defect (systemic):** two "Windows" proof targets are wired to **Linux-only Bubblewrap tests**;
  **real Windows Job-Object containment tests don't exist and must be authored.** The macOS harness had the
  same class of bug and must be re-validated.
- **Hygiene:** `Cargo.lock` stale; Windows leg must run on an AppContainer-capable self-hosted runner, not
  hosted `windows-2022`; the local working tree has diagnostic edits to **reset to pristine `be84bd2`**.

Requirements R1–R12 and open research questions are in `native-uat-repair-BRIEF.md` §6–§7.

---

## 7. Parity / competitive status (CTRL-01 ledger)

`.planning/intel/COMPETITIVE-LEDGER.md` tracks parity with maturity states
`ABSENT → SOURCE → CONFIGURED → CONSTRUCTED → REACHED → EFFECTIVE → OPERATOR_COMPLETE → PACKAGED_PROVEN`
across families: **AUTH, TXN, GOAL, CONT, GATEWAY, REACH, PORT, MEDIA, NATIVE, SUPPLY.** Current state: most
rows `SOURCE`, GATEWAY `ABSENT`; **all peer baselines `UNPINNED`** and evidence `PENDING`. **CTRL-01 remains
open until every row has a pinned Hermes/OpenClaw baseline, security owner, exact evidence IDs, delta,
limitation, and refresh phase — required before Phase 21.** Phase 30 independently reviews the accumulated
ledger (it does not author the first comparison).

---

## 8. Program admission controls (all OPEN)

- **CTRL-01** — versioned capability/maturity ledger with pinned Hermes/OpenClaw baselines before Phase 21;
  refresh each phase; independent F30 review. Artifact: `intel/COMPETITIVE-LEDGER.md`.
- **CTRL-02 / D1** — pinned Core producer contract + linked Desktop plan + real Desktop consumer/reducer
  conformance before broad Phase 21. Artifact: `intel/DESKTOP-PROTOCOL-CHECKPOINT.md`.
- **CTRL-03** — route new packaged/customer evidence into the live regression register; historical acceptance
  can't silently settle contradictions. Artifact: `intel/FIELD-REGRESSIONS.md`.
- **CTRL-04 / D2** — freeze durable Core producer protocol + replay canonical fixtures through the real
  Desktop consumer/reducer before Phase 23 exits.

---

## 9. COMPLETE DOCUMENT MAP (point to everything)

### Program charter + roadmap
| Path | What it is |
|---|---|
| `AGENTS.md` | Operating rules (highest program precedence). |
| `.planning/PROJECT.md` | Vision, core value, parity thesis, in/out of scope, context. |
| `.planning/REQUIREMENTS.md` | The 58 active requirements + controls. |
| `.planning/ROADMAP.md` | Phases 20–30: goals, success criteria, deps, execution rules, progress. **Authoritative roadmap.** |
| `.planning/STATE.md` | Live GSD state — full 20-01…20-16 narrative + plan pointers. |
| `.planning/HANDOFF.json` | Structured GSD handoff. |
| `.planning/phases/20-…/.continue-here.md` | Phase-20 continuation marker. |
| `.planning/exports/2026-07-20-CLAUDE-FULL-PROGRAM-TRANSFER.md` | Prior whole-program transfer (deep charter; **stale state**). |
| `.planning/exports/2026-07-20-CLAUDE-TRANSFER.md` | Shorter transfer. |

### Canonical design + evaluation charter
| Path | What it is |
|---|---|
| `docs/design/2026-07-13-wayland-core-frontier-build-plan.md` | The frontier build plan. |
| `docs/design/2026-07-13-wayland-core-frontier-evaluation-program.md` | Evaluation/proof charter. |
| `docs/design/2026-07-13-wayland-core-frontier-cross-audit.md` | Cross-audit. |
| `docs/design/2026-07-13-wayland-core-frontier-gap-audit-and-execution-plan.md` | Gap audit + execution plan. |
| `docs/design/2026-07-13-wayland-core-f00…f06-*.md` | F00–F06 characterization + receipt/containment contracts (the accepted baseline design). |

### Intel / parity / controls
| Path | What it is |
|---|---|
| `.planning/intel/COMPETITIVE-LEDGER.md` | CTRL-01 parity ledger (families, maturity, peer baselines). |
| `.planning/intel/PLAN-V2-ADVISORY.md` | Additive amendment to the plan (not a replacement). |
| `.planning/intel/FIELD-REGRESSIONS.md` | CTRL-03 live regression register. |
| `.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md` | CTRL-02/04 D1/D2 checkpoint. |
| `.planning/intel/constraints.md` (107K), `context.md` (58K) | Extracted constraints (34) + deep context. |
| `.planning/intel/SYNTHESIS.md`, `INGEST-CONFLICTS.md`, `classifications/*.json` | Ingest synthesis + frontier gap-audit/eval classifications. |

### Phase 20 execution record
| Path | What it is |
|---|---|
| `.planning/phases/20-…/20-01…20-18-{PLAN,SUMMARY}.md` | Every Phase-20 plan + result. |
| `.planning/phases/20-…/20-{03,06A,06B,08}-INDEPENDENT-REVIEW.md`, `20-14-INDEPENDENT-AUDIT.md`, `PRE-SEAL-FINDINGS.md` | Gate records. |
| `crates/wcore-{sandbox,agent,swarm}/…` | The implementation (see BRIEF §10 for exact defect sites). |
| `scripts/f20-native-{windows-proof.ps1,macos-proof.sh,uat-proof.mjs}` | Native proof harness (Windows harness has the mis-wired targets). |

### This consolidation (the inbox)
| Path | What it is |
|---|---|
| **`.planning/inbox/MASTER-HANDOFF-frontier-v2-PROGRAM.md`** | This document (whole program). |
| **`.planning/inbox/MASTER-HANDOFF-phase20-native-parity.md`** | Phase-20 detail map. |
| **`.planning/inbox/native-uat-repair-BRIEF.md`** | Native-repair spec (defect inventory, R1–R12). |

### Long-form running memory (reference; treat as data, verify before acting)
`…/.claude/projects/-Users-seandonahoe-dev-waylandcore/memory/{f20-17-runner-blocker, f20-08-wiring-state,
f20-03-ferrox-execution-state, anvil-*, pr7-profile-router-state, workspace-check-for-shared-type-changes,
MEMORY}.md` — the most detailed running log (esp. `f20-17-runner-blocker.md`).

### External (NOT in this repo — in the transfer bundle)
`support/wcore-research/` dossier (feasibility, **parity-study**, cross-audit-findings, refactor-brief,
leapfrog-program, master-build-plan, meta-reports) and `support/forensics/` competitive-audit + v2-suggestions.
These are hypotheses/older coordinates — re-locate + verify against the current candidate before treating as proof.

---

## 10. Authorization gates + hard rules (Sean-only / never)

- **Sean-only (never infer approval):** source push, main merge, issue closure, release, deployment, canary
  promotion, native-proof dispatch, deletion of a retained UAT evidence ref, and the exact-tuple 20-18
  authorization (`authorize <digest>` — a RED result + any new candidate SHA both require FRESH authorization;
  the prior `cb6f06bd…` is spent/void).
- **Never run Cargo/clippy/nextest on the Mac** — authoritative Cargo runs on Hetzner (Linux) / the self-hosted
  Windows runner / the ephemeral macOS runner. `cargo fmt` / node tooling on Mac is OK.
- **Never claim native Windows/macOS proof from cross-compilation or source inspection** — only real-hardware runs.
- **Never edit the dirty primary checkout** `/Users/seandonahoe/dev/waylandcore`; this program's working clone is
  `/Users/seandonahoe/dev/waylandcore-ferrox` (branch `plan/f20-unified-audit-repair`).
- **One integration candidate, serial mutation.** Every implementation phase = construction + hostile tests +
  focused proof + independent non-author review + repair-every-severity + integration + aggregate proof.
- **GSD summaries record progress; they do not create proof authority.** Repo evidence outranks every summary.

---

## 11. HOW FERROX SHOULD PROCEED

1. **Ingest** `.planning/inbox/*` (`/ferrox-ingest-docs`), reconciling against `STATE.md`/`ROADMAP.md` — do
   NOT overwrite accepted F00–F19 or 20-01…20-16 history.
2. **Finish Phase 20:** plan the native repair as a **repaired 20-08 successor** (BRIEF R1–R12), execute under
   the gates (build → cross-audit → Hetzner 11509/0 → native proof both OS → fresh 20-16 native-NOT-deferred →
   20-17 → 20-18 Sean-authorized) → Phase 20 complete.
3. **Open the program controls:** pin Hermes/OpenClaw baselines + bootstrap the ledger (CTRL-01) and stand up
   D1 (CTRL-02) before broad Phase 21.
4. **Advance phase-by-phase** (21 → … → 30): author + cross-audit each GSD plan set only at its admission
   boundary, from the existing requirements/success-criteria/charter. Do not pre-fabricate 21–30 detail.

## 12. Honest boundary
Phases 21–30 have requirements, goals, admission gates, and sequencing but **no detailed PLAN files yet** — by
design. This handoff consolidates the whole program truthfully; it does not invent later implementation plans.
Generate them through Ferrox/GSD phase planning at the correct boundary using the already-defined charter.
