---
phase: 21-child-authority-and-budget-inheritance
plan: "01"
subsystem: security
tags: [authority-inheritance, budget-rollup, admission-gate, cross-audit, census, json-stream, delegation]

requires:
  - phase: 20-transactional-delegated-mutation
    provides: isolated-mutation child worktrees, directory authority hardening, the ChildPolicySnapshot contract
  - phase: 20A-native-windows-macos-uat
    provides: the native Windows/macOS evidence and the amended four-plan / bounded-finding rules this phase inherits
provides:
  - The execution-time admission verdict for CTRL-01 (OPEN) and CTRL-02 (NOT-OPEN), each quoted from the real intel artifact and pinned by that artifact's live last-commit SHA
  - An eleven-dimension authority census with per-dimension seam, request channel, running mechanism, production reachability and a closed-set verdict
  - The five-seam grouping that bounds plan 21-02 to five case families instead of eleven workstreams
  - Eleven concrete widening attempts and eleven platform-qualified live surfaces, the sole authorised source of 21-02 corpus cases
  - A settled fan-out determination (DISTINCT-AND-COVERED) that adds no new authority knob
  - Three machine-readable SCOPE-LIMIT rows authorising 21-02, 21-03 and 21-04, taken by a four-way cross-audit over one shared bundle
  - The first canonical `wcore-contract digest` run, which the D1 document records as never having been performed
affects: [21-02, 21-03, 21-04, phase-22, CTRL-02, D1]

tech-stack:
  added: []
  patterns:
    - "Census verdict vocabulary ENFORCED / VACUOUS / UNREACHABLE / ABSENT, where VACUOUS and UNREACHABLE are the two states that read as safe and are not"
    - "NO-CHANNEL canary: any property currently protected by the absence of a request channel must carry a test that FAILS when a channel appears"
    - "Decide-do-not-park: a blocking checkpoint resolved by a four-way panel over one shared byte-identical bundle, with unedited captures and preserved dissent"

key-files:
  created:
    - .planning/phases/21-child-authority-and-budget-inheritance/21-01-ADMISSION-GATE.md
    - .planning/phases/21-child-authority-and-budget-inheritance/21-01-AUTHORITY-CENSUS.md
    - .planning/phases/21-child-authority-and-budget-inheritance/21-01-t3-panel/panel-prompt.txt
    - .planning/phases/21-child-authority-and-budget-inheritance/21-01-t3-panel/codex-sol.raw.txt
    - .planning/phases/21-child-authority-and-budget-inheritance/21-01-t3-panel/gemini-pro.raw.txt
    - .planning/phases/21-child-authority-and-budget-inheritance/21-01-t3-panel/kimi-k3.raw.txt
    - .planning/phases/21-child-authority-and-budget-inheritance/21-01-t3-panel/claude-adversarial.raw.txt
    - .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-01-t3-linux.log
  modified:
    - .planning/BACKLOG.md

key-decisions:
  - "Broad Phase 21 execution proceeds under a named scope limit — 3-1 majority of a four-way cross-audit panel; CTRL-02 NOT-OPEN was scope-limited around, never waived"
  - "The tool dimension's anti-amplification mechanism is ABSENT, not merely weak: Delegate's model-fillable `toolsets` array flows into a replacement whitelist that never consults the parent's tool set"
  - "PolicyGate is orphan code on the agent path — `set_policy_gate` has zero callers; the only production construction is the `mcp-serve` subcommand"
  - "Fan-out is DISTINCT-AND-COVERED. No `max_fan_out` knob is designed or added; concluding the existing knobs suffice IS the deliverable"
  - "The plan's own claim that `intersect_execution_budget` is the child-spawn primitive was confirmed WRONG by measurement (2 occurrences, one of them the definition, the caller on the restore path)"
  - "The plan's claim that egress defaults to AllowAllPolicy pass-through was CONTRADICTED by the source and the contradiction is recorded rather than reconciled away"
  - "21-03's repair authority is bound by the losing dissent: no repair may move the 40 generator source inputs or the 156-file contract corpus without re-running wcore-contract and re-pinning D1 section 3"

patterns-established:
  - "Measure the decision before the panel sees it: the named risk of the leading option was turned into four scripted legs, each emitting its own verdict line, so the panel argued over tool output rather than intuition"
  - "Seam grouping as a scope fence: eleven requirement words collapse to five seams, and the grouping is stated as a hard bound on the downstream plan"

requirements-completed: []

# Metrics
duration: 95min
completed: 2026-07-26
status: complete
---

# Phase 21 Plan 01: Admission Gate and Authority Census Summary

**CTRL-01 verified OPEN and CTRL-02 verified NOT-OPEN from the real artifacts; the eleven authority dimensions collapse to five seams of which eight are ENFORCED, two VACUOUS and one ABSENT; and a four-way panel authorised scope-limited execution 3-1 after the leading option's named risk was measured STABLE in four legs on real hardware — including the first canonical `wcore-contract digest` run and a live `wayland-core --json-stream` observation the D1 document records as never having been performed.**

Base SHA: `3d80f14662c3df9bd63aeb7ecffc144fe643a553`, branch `plan/f20-unified-audit-repair`.
Nothing under `crates/` was modified. No requirement was marked complete — this plan records
evidence only.

---

## 1. Admission gate — per-control verdict

| Control | Verdict | Source artifact | Artifact last commit | Artifact's own marker |
|---|---|---|---|---|
| CTRL-01 | **OPEN** | `.planning/intel/COMPETITIVE-LEDGER.md` | `d06a60513e1b41507ae41a7513e892ddb6cf6fc6` (2026-07-26 10:45:39 +0700) | Footer: *"CTRL-01 refreshed 2026-07-26 against Phase 20 seal `01a5b0ae` and Phase 20A seal `9821ef76`."* Self-declared date and git date agree. Not stale. |
| CTRL-02 / D1 | **NOT-OPEN** | `.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md` | `738977ee1f3e84a121855f4a98c874576c18abb7` (2026-07-23 20:51:04 +0700) | No internal version marker — it is a standing contract document, not a refreshed ledger. `.planning/intel/D1-CORE-PRODUCER-CONTRACT.md` (`65339c4e`, 2026-07-26 11:19:57) is the newer artifact reporting progress against it. |

**CTRL-01 evidence.** *"Both peer baselines are **PINNED** as of 2026-07-26"* — Hermes 0.17.0 at
`dbe734be` (pin source `git show dbe734be:pyproject.toml` line 10), OpenClaw 2026.6.2 at
`11a0ad10` (pin source `git show 11a0ad10:package.json` line 3); zero `UNPINNED`. An F03/F05
retroactive evidence map maps `F03-RECEIPT@1c644ccd` to AUTH-* and SUPPLY-* and maps all eight
F05 capability identities to rows. The file's own close condition is discharged clause by clause:
*"**Status: CLOSED for admission purposes — with two carried limitations, neither of which is a
missing external input.**"*

**CTRL-02 evidence and the precise unmet clause.** D1 is asymmetric. Its **Core half is
discharged** — `D1-CORE-PRODUCER-CONTRACT.md` pins SHA `b6936299…`, publishes three
`digest_named_bytes` digests, and records `302 tests run: 302 passed, 0 skipped` for
`wcore-protocol` at that SHA. Its **Desktop half is not**: §9 lists eleven items and closes
*"Until items 1–2 exist with a green run, D1 is not complete."* The unmet clause is precise —
the pinning **target** exists and is exact; the pinned **party** does not. There is no linked
Desktop plan referencing `b6936299` and no consumer/reducer conformance harness replaying the
serialized corpus through the real Desktop reducer. That half lives in another repository and
cannot be closed from this checkout.

**Neither control was worked on.** Closing them is separate work owned elsewhere.

---

## 2. The eleven-dimension census

| Dimension | Seam | Child-request channel | Mechanism that runs | Reachable | Verdict | Sev |
|---|---|---|---|---|---|---|
| provider | `spawner.rs :: resolve_durable_launch` | **none** — both tool schemas hardcode `provider: None` | resolver/inherit; **no intersection anywhere** | resolver yes, intersection does not exist | **VACUOUS** | HIGH |
| tool | `spawner.rs :: build_tool_registry` | **yes, LLM-driven** — `Delegate.toolsets` → `ForkOverrides.allowed_tools` | replacement whitelist over a fixed 6-tool array + read-only floor + isolated-mutation gate; **parent's tool set never read** | yes | **ABSENT** | **HIGH** |
| filesystem | `directory_authority.rs :: validate_path` | none (workspace derived, not requested) | `WorkspacePolicy::contained` + `SandboxedFs` + `validate_path` | yes | ENFORCED | — |
| egress | `wcore-egress/policy.rs :: install_global_policy` | none per-actor | `policy_from_config` → `with_default_policy`, enforcing by default | yes | ENFORCED | — |
| secret | `spawner.rs :: SecretDenyFs` | none | `SecretDenyFs` as inner VFS of every child registry | yes | ENFORCED | — |
| approval | `execution_policy.rs :: with_requested_approvals` | **none today** — `PolicySource::Child` is test-only | managed → ratchet; **non-managed → replacement, `Bypass` accepted verbatim** | resolver yes, child leg does not exist | **VACUOUS** | HIGH |
| depth | `execution.rs :: enter_agent` | `sub_budget(override_)`, every production caller `None` | ancestor rollup + `first_exceeded_reason` | yes | ENFORCED | — |
| fan-out | `spawner.rs :: active_child_permits` | breadth requestable; concurrency not | breadth cap at `spawn_tool.rs:183` + Arc-shared 20-permit semaphore | yes | ENFORCED | — |
| time | `execution.rs :: remaining_wall_time` | as depth | `minimum_remaining` across ancestors + rollup | yes | ENFORCED | — |
| token | `execution.rs :: record_tokens` | as depth | rollup to every ancestor | yes | ENFORCED | — |
| cost | `execution.rs :: record_cost` | as depth | rollup to every ancestor | yes | ENFORCED | — |

Eight ENFORCED, two VACUOUS, one ABSENT. Every seam citation was resolved against the live tree.

---

## 3. Was this plan's own budget claim confirmed or contradicted?

**CONFIRMED, by measurement.** `intersect_execution_budget` has exactly **2** occurrences in the
whole workspace — the definition at `execution.rs:743` and one call at `execution.rs:301`, on the
snapshot-**restore** path. `intersect_caps` is the same shape (`tracker.rs:1563` definition;
callers `:540`, `:1549`, both restore). Neither is on any child-spawn path. The child-spawn
mechanism is `sub_budget` plus ancestor rollup, exactly as the plan asserted. A repair aimed at
the intersection helper would change the wrong mechanism.

**CONFIRMED — the vacuity is in the channel, not the enforcement.** All five production
`sub_budget` callers pass `None`; the one parameterised wrapper, `begin_active_turn`, has one
production caller (`engine.rs:6164`) which also passes `None`. Only `tests/budget_test.rs` passes
`Some(..)`. But the ancestor rollup is a real mechanism that *would* refuse a wider child cap, so
F21-02 is not *wholly* vacuous — which is what makes the S1 corpus family meaningful rather than
ceremonial. Hence the mandatory NO-CHANNEL canary class.

**CONTRADICTED — the egress default.** The plan states the shipped default is `AllowAllPolicy`
pass-through. The source says otherwise: `AgentBootstrap::build` (`bootstrap.rs:556-560`) calls
`policy_from_config` and wraps the whole engine build in `with_default_policy`, and
`policy_from_config` returns `AgentEgressPolicy::enforcing(build_allowlist(config))` whenever
`config.security.enabled` — the default. Every `with_policy(AllowAllPolicy)` occurrence in the
workspace is inside a `#[cfg(test)]` module, including the two that look production. The egress
dimension is **(a) not per-actor and so not child-wideable**, not (b) or (c). The contradiction is
recorded, not reconciled away.

**`limit_for` — answered by search, not inference. NO.** No admission or budgeting decision reads
it. All five production sites render the `BudgetExceeded` payload *after* the decision was taken
by `first_exceeded_reason()` (directly, or via `MonitorAction::CancelBudget { reason }` which
originates at `orchestration/monitor.rs:182`). The leaf-fallback in `with_reason_state` is real
but inert, because the caller has already selected the reason and the fallback walks the same
order. Recorded as LOW-1 in BACKLOG so it is not rediscovered.

---

## 4. Seam grouping handed to 21-02

| Seam | Dimensions | Case family |
|---|---|---|
| S1 budget view + ancestor rollup | depth, time, token, cost | one parameterised rollup family + NO-CHANNEL canary |
| S2 spawn seam + child registry | tool, provider, filesystem, secret, fan-out | one child-construction family |
| S3 policy layer | tool (secondary) | reachability proof, not behaviour |
| S4 egress chokepoint | egress | one chokepoint family |
| S5 execution-policy resolver | approval | one ratchet family + NO-CHANNEL canary |

Eleven dimensions, five seams, five case families — each run on both the standalone and the
host-protocol surface and compared, for Success Criterion 3. 21-02 may not invent widening
attempts outside the census's eleven `WIDENING` rows.

---

## 5. Fan-out determination

**`FANOUT :: DISTINCT-AND-COVERED`.**

Fan-out and concurrency are genuinely distinct — different code, different numbers. Breadth is
capped at `spawn_tool.rs:183` against `topology.default_config().max_agents` (Spawn 5, Mesh 50,
Swarm/Fleet 100, clamped to Mesh's 50 on the relay path). Concurrency is capped by
`active_child_permits`, a `Semaphore::new(20)` built from a constant. So SAME-AS-CONCURRENCY is
wrong and was rejected.

**Bounded ADMITTED work:** `spawn_one_with_active_permit` awaits `acquire_owned()` on a semaphore
whose `Arc` is carried into every child spawner (`clone_for_spawn`, `spawner.rs:2168`), so a
child draws admitted children from the *parent's* 20, not a fresh 20. Bounded tree-wide at 20 and
not child-widenable.

**Bounded PENDING work:** `spawn_parallel_with_extras_origin` (`spawner.rs:1699-1711`) and
`spawn_parallel_with_per_task_extras_origin` (`:1856-1871`) `tokio::spawn` one task per config
**before** any permit is acquired. Tasks beyond the admitted 20 sit pending on `acquire_owned()`
holding a config, a cloned spawner and cloned extras. The semaphore says nothing about them; the
**breadth cap** does, refusing the request before any task is spawned. Pending work is bounded per
request at `cap` and tree-wide at `20 × cap`.

**COVERED, not GAPPED:** everything the pending set could amplify is already bounded — task count
and memory by the breadth cap, spend/time/depth by the S1 ancestor rollup. **No `max_fan_out` knob
was designed, proposed or added.** Both existing workspace uses of the term were checked and
rejected as non-authority: `EvolveParams.fan_out` is GEPA search width; `FLEET_FANOUT_THRESHOLD`
is a *routing* threshold that re-routes rather than refuses.

---

## 6. Findings

**HIGH — must be fixed or disproved with executable evidence downstream**

- **HIGH-1 (tool).** `DelegateTool`'s schema advertises a model-fillable `toolsets` array
  (`delegate.rs:283`); it flows verbatim into `ForkOverrides.allowed_tools` (`delegate.rs:220`);
  `build_tool_registry` (`spawner.rs:2396`) grants exactly what was named from a fixed 6-tool
  array and **never consults the parent's registry**. A parent restricted to Delegate +
  Read/Grep/Glob can hand its child `Bash`. `DelegateTool` is registered in the production
  bootstrap at `bootstrap.rs:2310`. Blast radius is bounded by three real mitigations the corpus
  must *measure*: empty `toolsets` defaults to read-only; anything beyond that forces an
  isolated-mutation worktree; `Delegate` is not in the child's array so a child cannot re-delegate.
- **HIGH-2 (provider).** No intersection mechanism exists at all. Holds today only because no
  shipped schema exposes the field.
- **HIGH-3 (PolicyGate orphan).** `set_policy_gate` has **zero callers** — two hits in the
  workspace, the doc comment at `engine.rs:2679` and the definition at `:4064`. Every engine
  `policy_gate` initialiser is `None`. The only production `PolicyGate::new` is
  `wcore-cli/src/main.rs:1170`, inside `TopCmd::McpServe`. The policy layer that would express
  parent/child intersection is never consulted by an agent session. Verdict on that mechanism:
  UNREACHABLE. This is the `wcore-permissions` orphan-code failure, repeated one version later.
- **HIGH-4 (approval).** `with_requested_approvals` (`execution_policy.rs:151-153`) replaces
  rather than ratchets in the non-managed branch: a requested `Bypass` is accepted verbatim.

**MEDIUM and below → `.planning/BACKLOG.md`, non-blocking:** MED-1 Windows TUI not drivable
(`pty_capture.rs` `#![cfg(unix)]`); MED-2 parent/child permit contention (liveness, explicitly not
amplification); MED-3 `EgressClient::with_policy` bypass route (no production site uses it);
LOW-1 `with_reason_state` leaf fallback (inert).

**OUT-OF-PHASE, routed to BACKLOG, NOT made into a fifth plan:** OOP-1 the `delegate_isolation`
F05 identity has not been re-gated after Phase 20 (the ledger assigns it to Phase 21, but it is an
F05 capability-gate re-run, not an authority proof, and the cap forbids a fifth plan); OOP-2
`wcore-permissions` has no inheritance model by design.

**Four-plan cap intact** — four `*-PLAN.md` files. No fifth plan created or proposed.

---

## 7. The authorization, verbatim

```
PANEL-DECISION :: proceed-scope-limited :: MAJORITY
PANEL-VOTE :: codex-sol :: proceed-scope-limited :: codex-sol.raw.txt
PANEL-VOTE :: gemini-pro :: proceed-scope-limited :: gemini-pro.raw.txt
PANEL-VOTE :: kimi-k3 :: proceed-scope-limited :: kimi-k3.raw.txt
PANEL-VOTE :: claude-adversarial :: hold :: claude-adversarial.raw.txt
SCOPE-LIMIT :: 21-02 :: PROCEED :: dual-surface corpus against the Core producer contract as pinned and measured; no Desktop consumer/reducer equivalence may be claimed
SCOPE-LIMIT :: 21-03 :: PROCEED :: bounded to the four Core-internal HIGH findings; NO repair may move the 40 generator source inputs or the 156-file corpus without re-running wcore-contract digest and check and re-pinning D1 section 3
SCOPE-LIMIT :: 21-04 :: PROCEED :: Core verdict only; any whole-Wayland or Desktop-consumer claim is reserved until CTRL-02 closes
```

3-1 strict majority, so basis `MAJORITY`; no tiebreak needed. Every recorded vote was verified
against the **last** `PANEL_POSITION=` line in that member's own capture. All three measured
vote-loss traps were handled: `--skip-trust` for gemini, absolute path plus **unanchored**
extraction for kimi's indented output, **last-match** extraction for codex's repeated block
(its capture carries the marker three times).

**`proceed-broad` was not available and no `WAIVER` row exists** — CTRL-02 measured NOT-OPEN and
that option's own `cons` remove it on a single NOT-OPEN verdict. The gate was scope-limited around,
not bypassed.

**The dissent lost the count and won a clause.** The adversarial member argued that the four legs
measured whether the contract has *already* drifted (a statement about the past) while the option's
risk is about the future — and, unanswered by any of the three majority transcripts, that 21-03's
authorized repair of HIGH-4 is itself the most likely *cause* of that drift, because HIGH-4 sits on
`EffectiveExecutionPolicy` and `execution_policy` is a versioned **sub-contract** of the Desktop
corpus with its own fixture and adversarial vectors. That is correct in mechanism. It does not
defeat `proceed-scope-limited`, only an unbounded version of it — which the member conceded in
writing — so it was answered by binding it into the `21-03` scope row rather than dismissed.

**Where the majority itself split:** codex-sol said 21-04 *"must hold"*; kimi-k3 said it *"proceeds
only as a Core-only claim"*. The executor adopted kimi's position, because no Phase 21 Success
Criterion requires the Desktop consumer — SC1 and SC2 are Core-internal and SC3 names the
*producer* surface, the discharged half — so blocking the verdict outright would leave the phase
unresolvable for a reason its own criteria do not ask for. Codex's stricter reading is recorded in
the gate document as a minority within the majority.

**Sean was not reached.** No deadlock: 3-1, all four members answered, nothing on the reserved list.

---

## 8. Live evidence — what was exercised on real hardware

Host `hetzner-dsm`, phase-dedicated worktree `/root/wayland-p21` created from `/root/wayland` so no
checkout another agent may hold was disturbed. Transcript pinned to the decision commit; first line
`RUN_SHA=3d80f14662c3df9bd63aeb7ecffc144fe643a553`. Full log:
`.planning/phases/21-child-authority-and-budget-inheritance/evidence/21-01-t3-linux.log`.

```
CONTRACT-DIGEST RESULT=MATCH RC=0
CONTRACT-CHECK RESULT=CURRENT RC=0
LIVE-READY RESULT=EMITTED RC=0
CORPUS-PIN RESULT=MATCH RC=0
MEASURED :: contract-drift :: STABLE
```

- **CONTRACT-DIGEST** — `cargo run -p wcore-protocol --bin wcore-contract -- digest` printed all
  three digests equal to D1 §3. **This closes a gap D1 records against itself**: §3.2 states
  *"This author did NOT run it"* of the canonical Rust reproduction, having substituted a
  throwaway Python script. This is the canonical run.
- **CONTRACT-CHECK** — `check` regenerated every artifact in memory and compared byte-for-byte to
  the checked-in corpus. Exit 0.
- **LIVE-READY — the real product was launched, not merely compiled.** `wayland-core --json-stream`
  was built and spawned against a hermetic `WAYLAND_HOME`; the `ready` event it emitted carried
  `name=wayland-desktop-core, major=1, minor=8, generator=wcore-desktop-contract-gen/11` and
  `fixture_digest=sha256:42f142ab…`, `schema_digest=sha256:e5d1744a…`,
  `source_inputs_digest=sha256:d8b1a8b5…` — identical to D1 §3 **and** to `manifest.json`. Phase
  20A shipped CI-green with nobody ever launching the binary; that was not repeated.
- **CORPUS-PIN** — the toolchain-free whole-corpus pin over all **156** files re-derived to
  `a39c1379…21e6`, matching D1 §3.3. Independently re-derived a second time on the Mac; agreed.

---

## 9. Deviations from plan

**None of substance. The plan executed as written.** Two things worth recording rather than
burying:

1. **`git grep` is unreliable on this Mac** — the local `rtk` shim intermittently rewrites
   `/usr/bin/git grep …` into an invalid `git rtk grep`, returning 0 rows. Task 2's
   `MEASURED :: intersect_execution_budget_occurrences :: 2` gate is written against `git grep`
   and failed for that reason on one run. The **claim** was verified three ways and is correct at
   **2**: a successful local `git grep` earlier in the session, `/usr/bin/grep -rcF` over `crates/`,
   and `git grep -cF` on `hetzner-dsm` where no `rtk` shim exists. This is a harness artifact, not
   a repo condition, and it is called out so a later reader does not mistake it for drift.
2. **HEAD moved under the plan mid-session** — another agent committed to this shared checkout
   between two reads (`f69d4253` → `3d80f146`). `BASE-SHA` was pinned to `3d80f146` *after* the
   move and before any measurement, so every artifact, the panel bundle and the host run are all
   pinned to one commit. `.planning/BACKLOG.md` was modified beyond the plan's `files_modified`
   list; that is required by the phase's own amended rule that MEDIUM and below are logged there.

---

## 10. Termination state

**State 2 — Complete, gate not open, scope-limited authorization.**

CTRL-02 verified NOT-OPEN; the panel authorized a scope-limited continuation naming, plan by plan,
exactly which of 21-02, 21-03 and 21-04 may proceed and under what limit; the limit is recorded as
three machine-readable `SCOPE-LIMIT` rows in `21-01-ADMISSION-GATE.md` §5.1, which those three
plans read instead of re-deriving the gate.

Nothing under `crates/` was modified, no fifth plan was created or proposed, and no requirement was
marked complete. Evidence recorded for CTRL-01, CTRL-02, F21-01 and F21-02; F21-01 through F21-04
close on the corpus and the attribution proof, not on an inventory.

## Self-Check: PASSED
