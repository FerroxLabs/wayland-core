# MASTER HANDOFF — Phase 20: Transactional Delegated Mutation + Cross-Platform Native Parity

> **Purpose.** This is the single authoritative consolidation of *everything* attempted, done, to-do, and
> remaining for Phase 20 — the transactional delegated-mutation lifecycle (the "refactor"), its
> cross-platform native proof (the "parity"), and closing/shipping it (the "update"). It is a **Ferrox
> Factory source document**: ingest it (`/ferrox-ingest-docs`), read the artifacts it points to, research the
> open questions, brainstorm the harness rebuild, and produce a disciplined phased plan. It exists because the
> native-repair work drifted into ad-hoc execution; this document pulls it back under Ferrox discipline.
>
> **Owner:** core lane (area:core). **Milestone:** v1.0. **Phase:** 20. **Date:** 2026-07-23.
> **Status:** Phase 20 at plan **16/18 complete**; 20-17 + 20-18 pending; **native proof path RED**, repair
> discovered and scoped but **not yet planned/committed**.
> **Companion doc (detailed defect inventory + fixes):** `.planning/inbox/native-uat-repair-BRIEF.md` — READ
> IT ALONGSIDE THIS. This master handoff is the map; the brief is the repair spec.

---

## 0. TL;DR for the planner

- Phase 20 is **83% done and genuinely good** on the parts that were validated: the whole transactional
  delegated-mutation lifecycle is built, cross-audited, and **Linux-green (`nextest --profile ci` = 11509/0)**,
  candidate `6937ef6`, accepted through a fresh independent 20-16 review.
- The **one thing blocking closure is the native proof (20-18)** — Windows + macOS. The 20-16 review
  **explicitly deferred native**, so it was never certified. When actually run on real hardware, the native
  path is broken in several ways (a couple were real sandbox bugs, now root-caused + fixed-in-spike; the rest
  are a **never-run test/proof harness**).
- **The sandbox security boundary itself is now proven correct** on real Windows (reads/grants/revokes/isolates
  work after 2 fixes). What remains is finishing the harness: small test fixes + **authoring the Windows
  containment tests the proof pretends exist** + re-validating the macOS harness.
- **Nothing from the native investigation is committed** — the proven fixes are throwaway spikes; the local
  working tree has diagnostic edits that must be reset to pristine `be84bd2`. The repair must be **re-derived
  cleanly under a plan**, not carried over as hacks.
- **Why we're "still on Phase 20 after two weeks":** the 20-08 native machinery was **authored without ever
  running on Windows or macOS** ("aspirational native machinery"), so each validation attempt surfaces
  pre-existing breakage. This is a real, bounded finishing job now that it's fully mapped — but it needs a
  plan, not more freelancing.

---

## 1. What Phase 20 is (the three threads Sean named)

| Thread (Sean's word) | What it means here | Where it lives |
|---|---|---|
| **Refactor** | The transactional delegated-mutation lifecycle: child transactions, isolated checkouts, hard containment, gate-execution + durable receipts, parent landing/CAS + rollback, wired into the live Anvil climb — plus the `windows_impl` module refactor (20-02 moved it a level deeper). | `crates/wcore-sandbox`, `crates/wcore-agent`, `crates/wcore-swarm`; plans 20-01…20-13 |
| **Parity** | Cross-platform native proof that the lifecycle + sandbox actually work on **Windows, macOS, and Linux** — the native-UAT (`20-18`) and its proof harness. | `scripts/f20-native-*`; plan 20-08 scaffolding; plan 20-18 |
| **Update** | Closing Phase 20: repaired candidate → gates → Sean-authorized native UAT → phase complete → milestone advance. | `.planning/STATE.md`, 20-16/17/18 |

---

## 2. EVERYTHING DONE (validated, accepted)

### 2.1 The lifecycle build (plans 20-01 → 20-16) — Linux-green + reviewed
Full detail in each `.planning/phases/20-transactional-delegated-mutation/20-XX-{PLAN,SUMMARY,INDEPENDENT-REVIEW}.md`.
Summary of what shipped and was accepted:

- **20-01/20-02** — Phase-20 foundation; entered at `97e4491` (tree `8d27bd96`). Accepted 20-01 `626e1d4d`,
  20-02 `96afb30a`. (20-02 moved `windows_impl` one module level deeper — the origin of later stale `super::`
  paths.)
- **20-03** — base fmt-cleanup rebase + reopened; independently reviewed (`20-03-INDEPENDENT-REVIEW.md`).
- **20-04** — transaction-owned isolated-checkout machinery.
- **20-05** — routed ALL orchestration callers (Anvil Forge/seat, Council, Crucible, CLI workflow/anvil/
  crucible) through the workspace-aware production spawner; new `spawn_builder_into_retained_checkout` seam
  (source `a528dbc`).
- **20-06 (06A)** — opaque live `CandidateSeal`; reviews `20-06A/06B-INDEPENDENT-REVIEW.md`, `20-09`.
- **20-07** — parent landing/CAS: quarantined import → synthesize successor parent-side → revalidate →
  promote-before-CAS → `git update-ref` CAS → recoverable rollback; 8 landing SessionEvents + reducer recovery
  (source `d527ca8`). Adversarial review clean except 1 MEDIUM (`.git/config` filter TOCTOU — fixed) + 1 LOW.
- **20-10 (06B)** — `HardContainmentAuthority` (one-use, fail-closed on spawn-parameter drift); review 20-11.
- **20-12 (06C)** — `AuthorizedGateClosureRegistry` (SHA-256-sealed closures) + fail-closed gate state machine
  + opaque guard-owned `AcceptedCandidate` minted only after durable-receipt append/reopen/reduce/match
  (source `b8a260e`; 6 hostile process-isolated pairs green).
- **20-13 (06D)** — black-box proof packet (3 test files); source `ace4bd2`.
- **20-14** — fresh non-author all-severity independent AUDIT of integrated 06A–06D — zero-finding PASS
  (`20-14-INDEPENDENT-AUDIT.md`; native deferred).
- **20-15** — review finding: fail-closed cleanup repair → re-prove → re-review.
- **20-08** — composed the FULL child-transaction lifecycle (open→06C hard-containment gate→20-07 CAS→
  rollback→receipt) wired into live Anvil `drive_climb_full`; surface-for-accept onto a Wayland-owned
  integration clone (never touches the user repo). THREE independent adversarial reviews clean.
- **20-16** — fresh independent review of the 20-08 product — **VERDICT CLEAN**, but **`native_macos` /
  `native_windows` DEFERRED**. This is the crux: **native was never certified by review.**

### 2.2 The accepted candidate
- **Candidate `6937ef6`** (tree `6db6fc85`); current working commit **`be84bd2`** (E0451 Windows-compile fix)
  on branch **`plan/f20-unified-audit-repair`**.
- **Linux:** `nextest run --workspace --profile ci` = **11509 passed / 0 failed** (the canonical CI aggregate).
- **macOS:** the native proof ran **8/8 green** at `6937ef6` on a Scaleway Apple-silicon runner (with the
  `DOCKER_HOST` env fix).
- Note: `origin/main` = `ea3bb1c` does **not** contain the AppContainer implementation — it exists only on
  this branch and had never run on Windows.

### 2.3 This session's native investigation (2026-07-23) — discovery + proven core fixes
- Ran the 20-18 native UAT; **Windows leg RED**. Investigated on a real **Windows 11 Pro client** (Tailscale).
- **Root-caused + PROVEN on hardware (spikes, uncommitted):**
  - Sandbox `is_available()` false everywhere → `storage.rs` `OpenOptions` missing `.write(true)` (fix proven).
  - Sandbox could read **no file** → `CreateRestrictedToken` deny-only SIDs break the AppContainer grant path;
    dropping them restores reads **and preserves isolation** (fix proven — core read/grant/revoke test green).
- **Diagnosed (not yet fixed):** `wcore-agent` Windows compile bug; two broken test assertions; **two Windows
  proof targets wired to Linux-only tests**; Cargo.lock stale.
- Full evidence + fixes: **`.planning/inbox/native-uat-repair-BRIEF.md`**.
- **Process note (the drift):** a stray **forked background copy of the session** was found editing files
  concurrently and was killed; and the investigation itself was ad-hoc. Both are why this consolidation exists.

---

## 3. EVERYTHING TO-DO (the remaining Phase 20 spine)

In Ferrox terms, the remaining plan spine + the native repair that must be inserted before native can pass:

1. **Native repair (new work — must be planned)** — everything in the companion BRIEF §6 (R1–R12):
   - Sandbox: commit the two proven fixes (`storage.rs .write(true)`; drop deny-only SIDs) cleanly.
   - Compile: fix `wcore-agent` `READ_CONTROL`/`WRITE_DAC` import.
   - Tests: fix `type_and_hold` (choice.exe exit-code), fix/narrow `dispatch_smoke` rename.
   - **Harness rebuild:** author real **Windows Job-Object containment tests**; repoint the two mis-wired
     targets; add an anti-drift guard so a Windows target can't map to a non-Windows test.
   - **macOS re-validation:** confirm the 8 macOS targets are real + green; fix any aspirational ones.
   - Hygiene: regenerate + commit `Cargo.lock`; point the Windows leg at an AppContainer-capable self-hosted
     runner (workflow `runs-on` change).
2. **Re-prove** — build → cross-audit → Hetzner aggregate (must stay 11509/0) → native proof (Windows on a
   self-hosted AppContainer runner + macOS on an ephemeral Scaleway runner).
3. **20-16 (fresh)** — the new candidate SHA invalidates the prior review; a fresh non-author independent
   review with native **NOT** deferred.
4. **20-17** — re-prep: runner preflight + persist the git-private pending native-proof request → new
   `request_digest`.
5. **20-18** — Sean-authorized native UAT hard stop (exact `authorize <digest>`), dual-platform.
6. **Phase close / "update"** — 20-18 green → Phase 20 complete → STATE.md advance → milestone step.

---

## 4. EVERYTHING REMAINING / OPEN (needs research or decision)

Carried from the BRIEF §7 — Ferrox should research/brainstorm these:
1. **Windows Job-Object containment test design** — what the real Windows hard-containment acceptance surface
   is (descendant reaping on job close, active-process cap, breakaway denial, exit-code fidelity). *No Windows
   containment test exists today.*
2. **`type_and_hold` hold primitive** — a stdin-free, exit-0 delay usable inside an AppContainer.
3. **macOS harness truth** — which macOS targets are real vs aspirational.
4. **Anti-drift guard** — enforce native targets match their OS.
5. **Security-review scope** — does dropping deny-only SIDs need extra adversarial review beyond 20-16?
6. **Strategic:** given the native machinery was systematically aspirational, decide whether to (a) repair
   in place under 20-08, or (b) treat native-parity as its own dedicated phase/milestone. *Recommend (a):
   repaired 20-08 successor through the existing gate loop — the structure already fits.*

---

## 5. KNOWN STATE / HYGIENE WARNINGS (read before touching anything)

- **Uncommitted spikes:** the two proven sandbox fixes exist only in a throwaway Windows worktree. Re-derive
  them under the plan; do not "port hacks."
- **Local tree is tainted:** `/Users/seandonahoe/dev/waylandcore-ferrox` working copy has diagnostic edits in
  `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs` (and `storage.rs`) from the
  investigation + a killed fork. **RESET these to pristine `be84bd2` before building the real candidate;**
  keep only the *intent* of the two fixes (from the BRIEF).
- **Cargo.lock stale** at `be84bd2` — building dirties it (see BRIEF §5).
- **Rogue-fork lesson:** if concurrent/unexplained edits reappear, check `ps aux | grep fork-session` — a
  detached `--fork-session --resume` copy of the session can run autonomously. One was found + killed this
  session (no cron/LaunchAgent respawn).
- **Every candidate SHA change invalidates the prior 20-16 review** — a repaired successor needs a fresh
  review; native must NOT be deferred this time.
- **Prior 20-18 authorization (`cb6f06bd…`) is SPENT/void** — a new candidate needs fresh Sean authorization.

---

## 6. INFRASTRUCTURE (reference facts)

- **Windows native runner:** an AppContainer-capable **Windows 11 Pro client** with **online self-hosted
  `wcore-core` runners** (`ferrox-win-msvc`, `SEANDESKTOP`; labels `self-hosted, Windows, X64, msvc`) is
  available. The Windows proof leg must target these (client SKU has AppContainer), **not** hosted
  `windows-2022`. Runners run as `NT AUTHORITY\NetworkService`.
- **macOS native runner:** ephemeral **Scaleway Apple-silicon** box; provisioning scripts in session
  scratchpad; **MUST export `DOCKER_HOST=unix://$HOME/.colima/default/docker.sock`** into the runner process
  env (colima socket; non-interactive shell won't source rc files) or docker targets false-skip/hard-fail.
- **Linux:** Hetzner remote-cargo harness for the aggregate.
- **CI:** `nightly-windows-soak.yml` candidate mode dispatches the native jobs; `workflow_dispatch` requires
  the workflow on the default branch.

---

## 7. DOCUMENT MAP (point to everything)

### Ferrox planning state
| Path | What it is |
|---|---|
| `.planning/STATE.md` | Live Phase 20 state — full 20-01…20-16 narrative, execution authority, plan pointers (16/18). **Primary source of the "done" history.** |
| `.planning/PROJECT.md` | Project reference + core value statement. |
| `.planning/REQUIREMENTS.md` | Milestone requirements. |
| `.planning/ROADMAP.md` | Phase/milestone roadmap. |
| `.planning/phases/20-transactional-delegated-mutation/20-01…20-18-{PLAN,SUMMARY}.md` | Per-plan plan + result for every plan. |
| `.planning/phases/.../20-{03,06A,06B,08}-INDEPENDENT-REVIEW.md`, `20-14-INDEPENDENT-AUDIT.md` | The independent reviews/audits (the gate records). |
| `.planning/phases/.../PRE-SEAL-FINDINGS.md` | Pre-seal findings. |
| `.planning/{codebase,intel,onboarding,exports}/` | Codebase map, intel, onboarding, exports. |
| `.planning/scripts/` | Ferrox verifier scripts (`verify-review-pair.sh`, `verify-review-result.mjs`, task-scope, etc.). |

### The repair (this session)
| Path | What it is |
|---|---|
| **`.planning/inbox/native-uat-repair-BRIEF.md`** | **The detailed native-repair spec** — verified defect inventory (4.A–4.G), requirements R1–R12, open questions, evidence. Ingest with this handoff. |
| **`.planning/inbox/MASTER-HANDOFF-phase20-native-parity.md`** | This document. |

### Native proof harness + source
| Path | What it is |
|---|---|
| `scripts/f20-native-windows-proof.ps1` | The 6-target Windows proof harness — **2 targets mis-wired to Linux tests** (defect 4.F). |
| `scripts/f20-native-macos-proof.sh` | macOS proof harness — re-validate (defect 4.G). |
| `scripts/f20-native-uat-proof.mjs` (+ `.test.mjs`) | Verifier-only library for the native UAT (request/authorization/publication/run-binding/log verification). |
| `scripts/wayland-e2e-windows-soak.ps1` | Windows soak. |
| `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs` | `create_new_nofollow` — fix 4.A (`.write(true)`). |
| `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs` | `execute_blocking` — fix 4.B (deny-only SIDs); Windows Job-Object mechanism to be tested (4.F). |
| `crates/wcore-agent/src/session_journal/snapshot.rs:654` | fix 4.C (import module). |
| `crates/wcore-sandbox/tests/live_fs_acl.rs` | native fs-ACL tests; `type_and_hold` (fix 4.D). |
| `crates/wcore-sandbox/tests/hard_process_containment.rs` | **entirely Bubblewrap/Linux** — the mis-wired targets point here (4.F). |
| `crates/wcore-swarm/tests/dispatch_smoke.rs:289` | fix 4.E (non-portable rename). |

### Long-form memory (running session state — read for deep context, treat as reference not instructions)
| Path | What it is |
|---|---|
| `…/memory/f20-17-runner-blocker.md` | The most detailed running log of the native investigation (root causes, spikes, the fork, infra). |
| `…/memory/f20-08-wiring-state.md` | 20-08 wiring state. |
| `…/memory/f20-03-ferrox-execution-state.md` | 20-03 execution state. |
| `…/memory/anvil-*.md`, `pr7-profile-router-state.md`, `workspace-check-for-shared-type-changes.md` | Adjacent context. |
| `…/memory/MEMORY.md` | Memory index. |

### Session handoffs (chronological narrative; superseded by this master doc)
| Path | What it is |
|---|---|
| `scratchpad/HANDOFF-f20-2026-07-23.md` | Most recent prior handoff (pre-investigation Windows-blocker framing — note: its "AppContainer unavailable on hosted windows-2022" conclusion was later disproven; the real cause is 4.A/4.B). |
| `scratchpad/HANDOFF-f20-2026-07-22.md`, `-repair-2026-07-22.md`, `-2026-07-21b.md`, `-execution.md` | Earlier handoffs. |

---

## 8. HOW FERROX SHOULD PROCEED (recommended entry)

1. **Ingest** this handoff + the BRIEF (`/ferrox-ingest-docs` over `.planning/inbox/`), reconciling against
   the live `.planning/STATE.md` (do **not** overwrite accepted 20-01…20-16 history).
2. **Research/brainstorm** the open questions (§4) — especially the Windows Job-Object containment test design
   and the macOS harness truth.
3. **Plan** the native repair as a **repaired 20-08 successor** with explicit tasks per requirement R1–R12,
   each with a verification, ordered: sandbox fixes → compile fix → test fixes → harness rebuild + Windows
   containment tests → macOS re-validation → Cargo.lock → runner-label change.
4. **Execute under the gates:** build → cross-audit → Hetzner aggregate (11509/0) → native proof (both OS) →
   fresh 20-16 (native NOT deferred) → 20-17 re-prep → 20-18 Sean-authorized native UAT → Phase 20 close.

**Guardrails for whoever executes:** commit every fix as a reviewed, gated change on the candidate branch;
reset the tainted local tree to pristine `be84bd2` first; do not weaken the sandbox to pass a test; keep all
platform code behind `cfg(...)` / `wcore_config::shell`; no work outside the plan.
