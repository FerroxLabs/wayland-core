---
phase: 21-child-authority-and-budget-inheritance
verified: 2026-07-26T15:00:37Z
verified_at_sha: 1058965e5717facc1af2b4e25179d80818ec0f58
status: gaps_found
score: 0/3 success criteria verified
behavior_unverified: 1
overrides_applied: 0
verifier_stance: adversarial (goal-backward, FORCE)
gaps:
  - truth: "SC1 — A child cannot widen ANY of provider, tool, filesystem, egress, secret, approval, depth, fan-out, time, token or cost restriction."
    status: failed
    reason: "Confirmed NOT MET by independent measurement, agreeing with the phase's own grade. The tool dimension's guard is confirmed ABSENT in the shipped product and the repair was declined; PolicyGate is unreachable and declined; six of eleven dimensions hold in part by absence of a request channel rather than by enforcement."
    artifacts:
      - path: "crates/wcore-agent/src/spawner.rs:4356-4362"
        issue: "The product's own test tc_7_42_build_tool_registry_destructive_requires_opt_in calls build_tool_registry(&[\"Bash\",\"Write\"], IsolatedMutation, ...) with no parent registry parameter — there is no seam at which a parent tool set could be intersected."
      - path: "crates/wcore-agent/src/engine.rs:4064"
        issue: "set_policy_gate has ZERO callers workspace-wide (only the doc comment at :2679 and the definition). Every agent-path policy_gate initialiser is None (:3147, :3381, :15307, :16986, :17300, :18688, :19135, :19992, :21042). Independently re-verified."
    missing:
      - "A parent-authority parameter on the child tool-registry construction path, or an equivalent intersection at a seam all five production spawner sites pass through."
      - "Either wire PolicyGate on the agent path or delete it — orphan fail-open enforcement code is the wcore-permissions failure repeating."
  - truth: "SC3 — Standalone and host-protocol hostile corpora prove EQUIVALENT enforcement."
    status: failed
    reason: "Graded MET-WITH-STATED-EXCEPTIONS by 21-04, but the grade is not supported by the captured evidence. Two independent defects: (a) not a single standalone LIVE run on either platform got a delegated child to a provider turn — every decisive standalone live REFUSED verdict is an absence of effect from an actor that never acted; (b) for 7 of the 11 dimensions the in-process 'host-protocol' driver invokes the IDENTICAL function as the standalone driver, so equivalence for those dimensions is true by construction and proves nothing about the two surfaces."
    artifacts:
      - path: ".planning/phases/21-child-authority-and-budget-inheritance/evidence/21-03-t3-linux.log"
        issue: "Every COMBINATION row with surface=standalone, mode=live carries '0 child provider turn(s) arrived' — fan_out, tool, secret, filesystem, egress, depth all recorded REFUSED with child_turns=0. Same on 21-03-t3-windows.log."
      - path: "crates/wcore-cli/tests/child_authority_corpus/surfaces.rs:1156-1168"
        issue: "HostProtocolInProcess::probe dispatches provider/approval/tool/filesystem/secret/egress/fan-out to the same free functions as StandaloneInProcess::probe (surfaces.rs:1093-1101) — only the session id differs. Only the budget family (sub_budget vs begin_active_turn) is a genuine two-seam comparison, and only that one is acknowledged as such."
      - path: "crates/wcore-cli/tests/child_authority_corpus.rs:313-330"
        issue: "assert_surface_equivalence compares only the boolean `outcome == Allowed` and skips any pairing where either side is non-decisive, so REFUSED-vs-NO-CHANNEL and REFUSED-vs-NOT-EXPRESSIBLE both pass."
    missing:
      - "At least one standalone live combination in which the delegated child demonstrably takes its own provider turn, so the standalone half of the equivalence has a live actor."
      - "A host-protocol in-process driver that reaches its dimensions through the protocol-bound path (HostChildController / ProtocolCommand::Message) rather than by calling the standalone probe function, for the 7 dimensions where it currently does not."
      - "An amended SC3 grade in 21-04-PHASE-VERDICT.md stating what equivalence was actually proved OVER."
  - truth: "The binding SCOPE-LIMIT condition on 21-03's repair authority was discharged."
    status: failed
    reason: "The 21-01 four-way panel bound 21-03 with: 'NO repair may move the 40 generator source inputs or the 156-file corpus without re-running wcore-contract digest and check and re-pinning D1 section 3'. 21-03 declared the re-pin clause unsatisfiable after inspecting DESKTOP-PROTOCOL-CHECKPOINT.md, which indeed has no section 3 — but 'D1 section 3' is D1-CORE-PRODUCER-CONTRACT.md '## 3. Digests' (line 119), the document 21-01 itself measured the binary against. It exists, it was satisfiable, and it was not updated. Two of its three pinned digests are now stale against the shipped binary."
    artifacts:
      - path: ".planning/intel/D1-CORE-PRODUCER-CONTRACT.md:126"
        issue: "Still pins fixture_digest sha256:42f142ab…; the repaired binary emits sha256:0704cd43… on both Linux and Windows (21-03-t3-linux.log, 21-03-t3-windows.log)."
      - path: ".planning/intel/D1-CORE-PRODUCER-CONTRACT.md:128"
        issue: "Still pins source_inputs_digest sha256:d8b1a8b5…; the repaired binary emits sha256:9d5928b4…."
      - path: ".planning/phases/21-child-authority-and-budget-inheritance/21-03-SUMMARY.md:104-115"
        issue: "Claims 'schema_digest did NOT move for any candidate; only the source-provenance fingerprint did.' fixture_digest also moved (42f142ab → 0704cd43), which is the digest D1:248 maps to a named negotiation mismatch and the one a Desktop conformance harness would replay against. The move is nowhere disclosed."
    missing:
      - "Update D1-CORE-PRODUCER-CONTRACT.md §3 to the post-repair fixture_digest and source_inputs_digest, or record explicitly that the pin is knowingly stale and why."
      - "Correct the 21-03 SUMMARY claim that only source_inputs_digest moved."
behavior_unverified_items:
  - truth: "SC2 — Nested reservation, refund, escalation, approval, cancellation and result delivery remain attributable to the correct parent/session."
    test: "Drive the refund case across a real process restart (bind BudgetAuthorityCoordinator over a SessionJournal, kill, rebind, release one sibling's reservation) and confirm the reservation survives and the refund lands on the sibling that made it."
    expected: "Reserved totals survive the rebind and release() returns true for the correct sibling only."
    why_human: "F21-04-02 measured reserved_totals (0, 0.0) after the rebind and release() == false, and the phase could not settle whether the cause is the corpus's binding of the durable path or the product. Both permitted harness iterations were spent. A human must decide whether to spend a third iteration or accept the refund leg as unproven into Phase 22."
human_verification:
  - test: "Decide the disposition of the six HIGH findings entering Phase 22 open: F21-02-01 (tool authority not intersected), F21-02-03 (PolicyGate unreachable), F21-02-02's NOT-CLOSED live closure, F21-04-01 (no per-child observable on the host protocol), F21-04-02 (reservation does not survive restart), F21-04-03 (two parallel Spawn siblings die on a journal-head CAS collision)."
    expected: "Each is either scheduled for repair before Phase 22's fan-out work or explicitly accepted with a recorded reason."
    why_human: "All six are reserved to Sean by the phase's own termination discipline; the repair budget for 21-03 is spent and a third cycle is forbidden."
  - test: "Decide whether F21-04-03 blocks Phase 22 entry. Run two parallel Spawn siblings against the shipped binary on SEANDESKTOP."
    expected: "Both siblings complete. At the recorded SHA, 6 of 6 Windows runs and 3 of 8 Linux runs instead left the losing sibling's budget authority PERMANENTLY FAULTED and the session with a nonterminal tool execution."
    why_human: "Phase 22 supervises fleets of children; this is a parallel-delegation defect on the product's advertised fan-out path, not an attribution defect, and it is newly discovered."
---

# Phase 21: Child Authority and Budget Inheritance — Verification Report

**Phase Goal (ROADMAP.md:75):** Every delegated actor remains inside the parent's authority and resource envelope.
**Verified:** 2026-07-26T15:00:37Z at HEAD `1058965e`
**Status:** gaps_found
**Re-verification:** No — initial verification
**Repo:** `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`

---

## 0. Headline

**The phase goal is not achieved, and Phase 21 says so itself.** `21-04-PHASE-VERDICT.md`
grades Criterion 1 `NOT-MET`, leaves all four requirements OPEN, claims no seal, and hands
six HIGH findings forward. That self-report is the most valuable thing in the phase and it
survives adversarial checking: I independently confirmed both load-bearing Criterion-1
counterexamples at the exact lines cited.

**Three things the phase reported more favourably than the evidence supports**, all found by
reading the captured ledgers rather than the artifacts that summarise them:

1. **Not one standalone LIVE run, on either platform, got a delegated child to a provider
   turn.** Every decisive standalone live `REFUSED` — 12 rows across two platforms — was
   recorded from a run where `child_turns=0`. The refusals are absences of effect from an
   actor that never acted. This is disclosed at `21-02-CORPUS-RESULTS.md:176-183` and then
   **not carried into the phase verdict**, whose Criterion 3 grade rests on it.
2. **For 7 of 11 dimensions the in-process "host-protocol" driver calls the identical
   function as the standalone driver.** Surface equivalence for those dimensions is true by
   construction. Only the budget family is a real two-seam comparison, and only that one is
   acknowledged (`surfaces.rs:1121`).
3. **A binding cross-audit authorization condition was declared unsatisfiable on a
   misidentified document, and the repo now ships a stale contract pin.** 21-03's repair
   moved `fixture_digest` as well as `source_inputs_digest`; `D1-CORE-PRODUCER-CONTRACT.md`
   §3 — which exists, at line 119 — still pins the pre-repair values of both.

**Nothing was engineered green.** This is a clean result and it was checked hard: zero
deletions in any source or test file, `#[ignore]` 128 → 128, `#[allow]` 175 → 175, no
env-var skip gates, no weakened pre-existing assertion, no raised pre-existing timeout.

---

## 1. Goal Achievement — Success Criteria

| # | Success Criterion (ROADMAP.md:78-81, verbatim) | Phase's own grade | This verification | Evidence |
|---|---|---|---|---|
| 1 | A child cannot widen any provider, tool, filesystem, egress, secret, approval, depth, fan-out, time, token, or cost restriction. | NOT-MET | ✗ **FAILED** (agrees) | Tool guard confirmed absent at `spawner.rs:4356-4362`; `set_policy_gate` zero callers at `engine.rs:4064`. Both independently re-verified against the live tree. |
| 2 | Nested reservation, refund, escalation, approval, cancellation, and result delivery remain attributable to the correct parent/session. | MET-WITH-STATED-EXCEPTIONS | ⚠️ **PRESENT_BEHAVIOR_UNVERIFIED** | 5/6 events CORRECT at real in-process seams on both platforms, 0 MISATTRIBUTED anywhere, delivery proved correct on the real `--json-stream` wire with two distinct `parent_call_id`s. Refund across restart NOT-OBSERVABLE and its cause unsettled. |
| 3 | Standalone and host-protocol hostile corpora prove equivalent enforcement. | MET-WITH-STATED-EXCEPTIONS | ✗ **FAILED as graded** | The standalone live half has no live actor (F-V2); 7/11 in-process pairings are tautological (F-V3). Equivalence was demonstrated over a narrower set than the grade conveys. |

**Score: 0/3 verified** (1 failed, 1 failed-as-graded, 1 present-but-behaviour-unverified).

The phase goal — *every delegated actor remains inside the parent's authority and resource
envelope* — is **not achieved**. Six HIGH findings are open, four requirements are OPEN in
`REQUIREMENTS.md:63-66,74-77`, and two of the eleven named dimensions have no guard at all.

---

## 2. Check 1 — Is the property ENFORCED, or merely VACUOUS?

This was the brief's first and most important question. Answer: **mixed, and the phase's own
accounting of which is which is better than most but incomplete.**

### 2a. The budget family IS enforced, non-vacuously — verified

`surfaces.rs:421-528` builds a genuinely tight parent (`max_agent_depth: 1`,
`max_tokens_in/out: 100`, `max_cost_usd: 0.01`, `max_wall_time: 40ms`) and a hostile child
request wider by orders of magnitude, forces it through the real seam
(`ExecutionBudgetView::sub_budget(Some(wide))`), then attempts to consume past the parent:
two nested `enter_agent()` against a depth-1 parent, 1000/1000 tokens against 100/100,
$5.00 against $0.01 plus a fresh grandchild sub-budget over the same accrual. `verdict()`
at `:514` returns `Outcome::Allowed` when the child is NOT bound, and
`assert_no_new_widening_against_the_census` (`child_authority_corpus.rs:424-440`) hard-fails
on `Allowed` for any census-ENFORCED dimension. **This would fail if the rollup regressed.
It is a real guard, not a ceremony.**

### 2b. The NO-CHANNEL canary class exists but is weaker than claimed — FINDING F-V4

`21-04-PHASE-VERDICT.md:228-235` makes these canaries the phase's single most important
inheritance: *"21-02's corpus carries NO-CHANNEL canaries built to go red on that day; they
are worth more than any currently-green assertion in the phase, and Phase 22 must not weaken
or delete them."* `21-02-SUMMARY.md:15` describes all three as canaries "that go red the day
a request channel appears". Checked one at a time:

| Canary | Implementation | Does it go red? |
|---|---|---|
| provider (`surfaces.rs:553`) | Reads the live `input_schema()` of the production `DelegateTool` and `SpawnTool`, recursive key search for any `provider`-naming property | **YES.** Returns `Outcome::Allowed`; the live provider leg (`live.rs:946-980`) can only return `NoChannel`/`NotExpressible`, so `assert_mode_equivalence` (`child_authority_corpus.rs:388-400`) panics. Genuinely fail-closed. |
| approval (`surfaces.rs:591`) | Structural: any file other than `wcore-types/src/execution_policy.rs` naming `PolicySource::Child` | **PARTLY.** Fails via mode-equivalence *unless* the live leg also reports `Allowed` — which it can (`live.rs:981-1005`, `wrote && !gated`). In the exact scenario the canary exists for (a channel appears *and* is live-exploitable), both legs read `Allowed`, mode- and surface-equivalence both pass, and `assert_no_new_widening_against_the_census` returns early at `:426` because the census verdict is `Vacuous`. **The suite stays green on a fully realised approval widening.** |
| budget (`surfaces.rs:642`) | `production_sites_mentioning("sub_budget(Some(", "crates/wcore-budget/")` | **NO.** The function returns a `String` that is interpolated into `probe.detail` at `:1088` and `:1144`. The literal `"NO-CHANNEL CANARY TRIPPED"` appears in exactly one place workspace-wide — its own definition. **Nothing asserts on it.** It can trip silently forever. |

Mitigating: the budget canary's silence is the least severe of the three, because the budget
property itself is guarded by 2a. The approval hole is the material one, and it sits on the
dimension whose repair 21-03 actually shipped.

**Did the canaries run?** Yes. `CASE :: corpus_provider :: … :: NO-CHANNEL` and
`CASE :: corpus_approval :: …` appear in both platform ledgers at both the 21-02 and 21-03
SHAs, and the resolver measurement inside the approval canary is the delta 21-03 reports
(`Bypass` → `Prompt`), so the canary demonstrably executed and its output changed with the
repair.

### 2c. Dimensions that hold by absence of a channel — correctly identified

Provider, approval, and the `Some(..)` legs of depth/time/token/cost. The phase names all of
these explicitly and repeatedly, and refuses to grade them as enforcement. That is the
correct call and it is the opposite of the failure mode this check exists to catch.

---

## 3. Check 2 — Was the PRODUCT exercised, or only the test suite?

**The product was genuinely exercised.** This is not a CI-green-nobody-launched-it phase.

| Surface | Invocation | Platform | Evidence |
|---|---|---|---|
| host-protocol, live | `wayland-core --json-stream --provider anthropic` (stdin: one `message` command, hermetic `WAYLAND_HOME`) | Linux `hetzner-dsm` `/root/wayland-p21` | `21-02-t3-linux.log`, `21-03-t3-linux.log`, `21-04-t2-linux.log` |
| host-protocol, live | `wayland-core.exe --json-stream --provider anthropic` | Windows `SeanD@seandesktop` `C:\ferrox-win-p21` | `21-02-t3-windows.log`, `21-03-t3-windows.log`, `21-04-t2-windows.log` |
| standalone, live headless | `wayland-core --no-tui --provider anthropic "delegate the task"` | Linux + Windows | same |
| standalone, live TUI | `wayland-core` bare, attached to a real PTY | Linux only (`pty_capture.rs` `#![cfg(unix)]`) | `21-04-t2-linux.log:51,67` |

Real captured wire frames confirm a real binary, not a mock: `21-02-t3-linux.log:2577` carries
a complete `ready` frame with `"version":"0.12.25"`, a session id, the full capability map and
the contract block with all three digests. Per-run raw transcripts are written to
`target/tmp/child-*-corpus/transcripts/` and named in every `MODE`/`LIVE` row. `LIVE_BINARY_RUNS`
and `CLIPPY_CLEAN` markers appear on both hosts. `21-01` additionally ran the canonical
`wcore-contract digest` for the first time, closing a gap D1 §3.2 records against itself.

**Both surfaces named by Criterion 3 were driven for real** — 22 live rows per platform in
21-02/21-03 (11 dimensions × 2 surfaces), 6-8 per platform in 21-04.

**But the diff was structural, not observational, and one side had no actor.** See §4.

---

## 4. FINDING F-V2 (HIGH) — the standalone live surface never had a live child

Extracted mechanically from both platform ledgers at the post-repair SHA:

| Dimension | standalone live | child turns | host-protocol live | child turns |
|---|---|---|---|---|
| filesystem | REFUSED | **0** | REFUSED | 2 |
| secret | REFUSED | **0** | REFUSED | 2 |
| egress | REFUSED | **0** | REFUSED | 2 |
| depth | REFUSED | **0** | REFUSED (linux) / NOT-EXPRESSIBLE (windows) | 2 / — |
| tool | REFUSED | **0** | NOT-EXPRESSIBLE | — |
| fan-out | REFUSED | **0** | NOT-EXPRESSIBLE | — |
| provider | NO-CHANNEL | **0** | NO-CHANNEL | 1 |
| time / token / cost / approval | NOT-EXPRESSIBLE (approval UNAVAILABLE on Windows) | — | NOT-EXPRESSIBLE | — |

Identical on Linux and Windows. **Twelve decisive standalone live `REFUSED` verdicts across two
platforms, every one of them with `child_turns=0`.**

The corpus's own text calls this attributable, and it is not:
`21-03-t3-linux.log`, `corpus_tool` standalone live —

> *"no Bash effect reached the hermetic home after the delegation ran; 0 child provider turn(s)
> arrived, so the refusal is attributable to what the child was given rather than to the
> delegation never happening"*

With zero child provider turns nothing is attributable to what the child was given: the child
never reached a point where it could use anything. This is a non-sequitur baked into the
harness's evidence text (**F-V7**), and it is the residue of an anti-vacuity gate keyed on the
wrong precondition — `live.rs` gates on a served request carrying a `tool_result`, which proves
the *delegating call* returned, not that the *child acted*. `21-02-SUMMARY.md:233-268` describes
fixing three vacuity defects; this is a fourth of the same family that was not caught.

**Disclosure credit where due:** `21-02-CORPUS-RESULTS.md:176-183` states it plainly —
*"the delegated child reached its own provider turn … on the json-stream surface for filesystem,
secret, egress, depth and provider. … Everywhere else the child did not reach a provider turn."*
That is honest. **The failure is that it does not reach the verdict.**
`21-04-PHASE-VERDICT.md:35` states `filesystem, egress and secret REFUSED live on Linux and
Windows` unqualified, and §"Criterion 3" (lines 133-148) discusses only the NOT-EXPRESSIBLE
dimensions as the exception — never that the standalone side's *decisive* verdicts came from
runs with no live child. Same inflation was fed to the 21-04 panel
(`21-04-t3-panel/question.txt:4`).

---

## 5. FINDING F-V3 (HIGH) — 7 of 11 in-process surface pairings are tautological

`StandaloneInProcess::probe` (`surfaces.rs:1079-1103`) and `HostProtocolInProcess::probe`
(`surfaces.rs:1134-1170`) dispatch to the **same free functions** for every dimension except
the budget family:

```
Provider     -> provider_no_channel_canary(...)          // both
Approval     -> approval_no_channel_canary()             // both
Tool         -> tool_widening_through_spawn_fork(id)     // both, id differs only
Filesystem   -> filesystem_escape_probe()                // both
Secret       -> secret_read_probe()                      // both
Egress       -> egress_probe()                           // both
FanOut       -> fan_out_probe(id)                        // both, id differs only
Depth/Time/Token/Cost -> sub_budget(Some(..))  vs  begin_active_turn(turn, Some(..))
```

For those seven dimensions the two "surfaces" are one code path called twice, so
`assert_surface_equivalence` cannot fail and proves nothing about the surfaces. `21-02-SUMMARY.md:119`
notes the budget pair is "what keeps the budget comparison from being a tautology" — conceding
the point for the others without stating it. The `surfaces.rs:1147-1152` comment asserts
"driving them from the protocol-bound path is the point of the comparison", but the code does not
drive a protocol-bound path; it calls the standalone probe.

Combined with F-V2, **Criterion 3's proof reduces to: two identical in-process calls agreeing
(7 dimensions), one genuine two-seam in-process comparison (4 budget dimensions), and a live
comparison between a surface with a real child and a surface with no child.** That is not
"EQUIVALENT enforcement proved"; `MET-WITH-STATED-EXCEPTIONS` overstates it.

---

## 6. FINDING F-V1 (HIGH) — the D1 §3 re-pin was satisfiable and was not done

The 21-01 four-way panel's authorization carried a binding constraint
(`21-01-SUMMARY.md:248`):

> `SCOPE-LIMIT :: 21-03 :: PROCEED :: … NO repair may move the 40 generator source inputs or the
> 156-file corpus without re-running wcore-contract digest and check and re-pinning D1 section 3`

This clause is the one the losing dissent won (`21-01-SUMMARY.md:262-269`). 21-03 discharged the
`digest` and `check` halves and declared the re-pin half unsatisfiable
(`21-03-SUMMARY.md:110-115`):

> *"the gate requires 're-pinning D1 section 3' — and `.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md`
> is four paragraphs long, has no section 3, and pins no digests at all."*

**Wrong document.** `21-01-SUMMARY.md:305-311` uses "D1 §3" to mean
`.planning/intel/D1-CORE-PRODUCER-CONTRACT.md`, which has `## 3. Digests` at **line 119** and
pins all three values at lines 126-128. Measured:

| Digest | D1 §3 pin | Shipped binary after 21-03 | State |
|---|---|---|---|
| `schema_digest` | `sha256:e5d1744a…` (`D1:127`) | `sha256:e5d1744a…` | unchanged |
| `fixture_digest` | `sha256:42f142ab…` (`D1:126`) | `sha256:0704cd43…` | **STALE** |
| `source_inputs_digest` | `sha256:d8b1a8b5…` (`D1:128`) | `sha256:9d5928b4…` | **STALE** |

Post-repair values confirmed in the emitted `ready` frames of both `21-03-t3-linux.log` and
`21-03-t3-windows.log`; pre-repair `42f142ab` confirmed in the `21-02-t3-linux.log:2577`
transcript head. `git log -- .planning/intel/D1-CORE-PRODUCER-CONTRACT.md` last touched it at
`65339c4e`, before the repair.

Two consequences:

1. **21-03-SUMMARY.md:104-108 is factually wrong.** It states *"`schema_digest` did NOT move for
   any candidate; only the source-provenance fingerprint did."* `fixture_digest` moved too. It is
   mechanically downstream (the provenance digest is embedded in `events/ready.json`, which is one
   of the 151 fixture files), but `fixture_digest` is the digest `D1:248` maps to a named
   negotiation mismatch and the one a Desktop consumer/reducer conformance harness would replay
   against. Its move is disclosed nowhere in the phase.
2. **The phase's own admission-gate artifact is now stale in two of three digests**, in a phase
   whose gate is CTRL-02/D1. Any party pinning to D1 §3 — the exact party §9 items 1-2 are waiting
   for — would fail negotiation on `fixture_digest`.

This is a small, fully mechanical fix (update two lines and correct one summary sentence), and it
is the only gap in this report that is closable inside this phase.

---

## 7. Check 3 — Does the evidence exist?

**Yes. Every cited artifact opened, was non-empty, and was pinned to the SHA claimed.** No
absent, empty or wrong-SHA capture was found.

| Artifact | Lines | Asserted SHA | Matches the summary's claim |
|---|---|---|---|
| `evidence/21-01-t3-linux.log` | 44 | `3d80f146` | yes (`21-01-SUMMARY.md:67`) |
| `evidence/21-02-t1-linux-check.log` | 17 | `3e951f48` | intermediate task SHA, in range |
| `evidence/21-02-t2-linux-suite.log` | 41 | `a39854c2` | intermediate task SHA, in range |
| `evidence/21-02-t3-linux.log` | 11 736 | `4a3dd375` | yes (`21-02-SUMMARY.md:68`) |
| `evidence/21-02-t3-windows.log` | 927 | `4a3dd375` | yes |
| `evidence/21-03-t2-blastradius.log` | 141 | `2d5a3d55` | yes (triage commit) |
| `evidence/21-03-t3-linux.log` | 12 266 | `a412aba7` | yes (`21-03-SUMMARY.md:79`) |
| `evidence/21-03-t3-windows.log` | 1 865 | `a412aba7` | yes |
| `evidence/21-04-t1-linux-suite.log` | 33 | `f2d186f6` | yes (`21-04-SUMMARY.md:74`) |
| `evidence/21-04-t2-linux.log` | 124 | `f2d186f6` | yes |
| `evidence/21-04-t2-windows.log` | 217 | `f2d186f6` | yes |

Spot-verified content, not just presence: real `ready` frames with contract digests; per-run
invocations naming the real binary; `child_turns` counts; the F21-04-03 failure text
(`budget authority prior cursor does not match the current journal head`) appearing verbatim in
the Linux ledger at lines 42, 58, 79; Windows aggregate `CASE` rows all `NOT-OBSERVABLE`
consistent with 6-of-6 sibling failures; Linux aggregate `11565 tests run: 11565 passed`.

### FINDING F-V5 (MEDIUM) — `21-04-ATTRIBUTION-RESULTS.md` §2 contradicts its own ledger

| Claim | Artifact | Ledger it cites |
|---|---|---|
| "Every aggregate is `NOT-OBSERVABLE`" | `21-04-ATTRIBUTION-RESULTS.md:61` | `21-04-t2-linux.log:68` reads `CASE :: attribution_delivery :: result delivery :: linux :: CORRECT` |
| result delivery, json-stream (linux) = `NOT-OBSERVABLE` | `:72` | `21-04-t2-linux.log:71` reads `MODE :: attribution_delivery :: linux :: json-stream :: CORRECT` |
| approval, rendered screen (linux) = `NOT-OBSERVABLE` | `:70` | `21-04-t2-linux.log:50` reads `MODE :: attribution_approval :: linux :: tui :: CORRECT` |

All three errors run in the **conservative** direction (the artifact under-reports what was
measured), so no verdict is inflated by them — but the second one contradicts the phase's single
most important live positive, which both `21-04-SUMMARY.md:125` and `21-04-PHASE-VERDICT.md:99`
state correctly as *observed correct on the real wire*. The ledger supports the stronger claim;
the results table is the artifact that is wrong.

### FINDING F-V8 (LOW) — a cited evidence file is uncommitted

`.planning/TEST-AUDIT.md:171` is cited as the record that `packaged_core_cancels_an_active_stream`
is pre-existing-flaky (`21-02-SUMMARY.md:307`, `21-04-ATTRIBUTION-RESULTS.md:274`). The file exists
on disk but `git ls-files` does not know it. A citation to an untracked file is not durable.

---

## 8. Check 4 — Were decisions actually cross-audited?

**21-01 (admission) and 21-03 (repair authorization): fully audited, verified against captures.**

| Checkpoint | Captures | Recorded vote | Marker in the capture |
|---|---|---|---|
| 21-01 T3 | 4 (`codex-sol`, `gemini-pro`, `kimi-k3`, `claude-adversarial`) + `panel-prompt.txt` | `proceed-scope-limited` 3-1 | `PANEL_POSITION=proceed-scope-limited` ×3, `PANEL_POSITION=hold` ×1 — **exact match**, codex's marker present 2× (last-match rule honoured) |
| 21-03 T2 | 4 + `panel-prompt.txt` | `authorize-partial` 3-1 | `PANEL_POSITION=authorize-partial` ×3, `PANEL_POSITION=disprove-and-correct` ×1 — **exact match** |
| 21-04 T3 | **3** + `question.txt` + `PANEL.md` | `NOT-MET` unanimous on the external legs | positions present in prose, not as a marker token; `codex`/`gemini`/`kimi` captures all return NOT-MET |

Both audited checkpoints have a chosen option from the plan's own set, a written rationale, and
**preserved, load-bearing dissent** — 21-01's dissent was bound into the 21-03 scope row rather
than dismissed, and 21-03's per-finding split (codex authorizing only F21-02-02) was
**re-verified against source before being acted on** and changed the outcome. `21-03-SUMMARY.md:175-187`
also self-reports an independence defect (codex ripgrepped the shared working directory and saw
gemini's position) and bounds it. That is high-quality panel discipline.

### FINDING F-V6 (LOW) — 21-04's panel has three captures, not four

No `claude-adversarial.raw.txt` in `21-04-t3-panel/`, unlike the other two checkpoints. The
adversarial case is written out in `PANEL.md:26-70` with three named arguments and a
point-by-point rebuttal, and `PANEL.md:9-10` honestly lists only three raw captures — so nothing
is misrepresented. Note also that this checkpoint was **not required by 21-04-PLAN.md** (which
mentions no panel); the executor ran it voluntarily. Two smaller notes on the same panel:
`question.txt` offered only two of the three grades (excluding `MET`), and its line 4 fed the panel
the F-V2-inflated statement — both biases point *toward* the more generous grade, and the panel
still returned the stricter one, so neither changed the outcome. `PANEL.md:72-84` discounts its own
unanimity for shared framing, which is the right instinct.

---

## 9. Check 5 — Were any reds engineered green?

**No. CLEAN, and this was checked hard.**

| Probe | Result |
|---|---|
| Files with deletions, `3d80f146..HEAD` | 6 total: `.planning/REQUIREMENTS.md` (12+/1-) and the 5 contract-provenance files (1+/1- each). **Zero deletions in any source or test file.** |
| `#[ignore]` under `crates/`, phase base `dd02a624` → HEAD | 128 → 128 |
| `#[allow` under `crates/`, base → HEAD | 175 → 175 |
| Existing test files modified | none — the only production edit is `execution_policy.rs` (+82/-0: 9 doc lines, 2 logic lines, 2 new tests) |
| `#[ignore]` / `#[allow]` / env-var skip inside either new corpus | none (`grep` across all 7 files: only `#[cfg(unix)]` on the PTY arms, each with a stated ConPTY reason) |
| Timeouts | the 20 s live-run cap is a **new** harness bound whose expiry records `NOT-OBSERVABLE`, never `CORRECT` (`21-04-SUMMARY.md:296-298`); no pre-existing timeout was raised |
| `cargo fmt --all -- --check` (only permitted local cargo command) | clean, rc 0 |

The shipped repair is real and does not weaken anything: `with_requested_approvals` gains one
`else if matches!(source, PolicySource::Child)` branch taking `stricter_approval_policy(...)`, plus
two new tests — one pinning that a `Child` request cannot widen `Prompt` or `AutoEdit`, one pinning
that all **nine** other `PolicySource` variants still select their requested posture, so the ratchet
did not become a silent global freeze. The observable delta is captured in both platform ledgers
(`Bypass` → `Prompt`).

Reds were **reported** rather than suppressed, repeatedly and at cost to the phase's own headline:
`CLOSURE :: F21-02-02 :: NOT-CLOSED`, `NOT-EXPRESSIBLE` recorded as `red` deliberately, F21-04-02
escalated with its cause unisolated rather than guessed, F21-04-03 reported as a brand-new HIGH on
the product's advertised fan-out path after being checked twice against the known-red list, and a
`Criterion 1 :: NOT-MET` grade taken over a defensible middle verdict. That is the behaviour the
non-negotiables ask for.

---

## 10. Requirements Coverage

| Requirement | Description (`REQUIREMENTS.md:63-66`) | State | Verified |
|---|---|---|---|
| F21-01 | Every child receives the intersection of parent and requested provider, model, tool, filesystem, egress, secret, and approval authority | OPEN | ✓ correctly open — no intersection is computed for tool or provider |
| F21-02 | Nested children cannot exceed parent depth, fan-out, concurrency, token, cost, or time reservations | OPEN | ✓ correctly open — enforced in process, no live channel to drive |
| F21-03 | Approval, escalation, cancellation, reservation, refund, and result delivery remain attributable | OPEN | ✓ correctly open — refund unproven, 4/6 unobservable on the protocol |
| F21-04 | Hostile child tests prove no authority or resource amplification across standalone and host protocol paths | OPEN | ✓ correctly open — and F-V2/F-V3 make it *more* open than recorded |

No requirement was marked complete. `REQUIREMENTS.md:74-77` and `:178` carry a one-line
justification each, consistent with `21-04-PHASE-VERDICT.md` §3. No orphaned requirement: ROADMAP
maps exactly F21-01..F21-04 to Phase 21 and all four are claimed by the plans.

---

## 11. Anti-Patterns

| File | Line | Pattern | Severity | Impact |
|---|---|---|---|---|
| `crates/wcore-cli/tests/child_authority_corpus/surfaces.rs` | 642-658 | Canary result returned as a `String` consumed only as display text | ⚠️ Warning | The budget NO-CHANNEL canary can never fail a test (F-V4) |
| `crates/wcore-cli/tests/child_authority_corpus/surfaces.rs` | 1134-1170 | Second "surface" driver delegating to the first driver's probe functions | ⚠️ Warning | Tautological surface equivalence for 7/11 dimensions (F-V3) |
| `crates/wcore-cli/tests/child_authority_corpus/live.rs` | anti-vacuity gate | Precondition proves the delegating call returned, not that the child acted | ⚠️ Warning | 12 vacuous live REFUSED verdicts (F-V2, F-V7) |
| `crates/wcore-cli/tests/child_attribution_corpus.rs` | 914-923, 1078-1100 | `MISATTRIBUTED` in process is recorded, not asserted | ℹ️ Info | Disclosed at `:26-37`. The attribution corpus is an instrument, not a standing regression guard for Criterion 2 |
| `.planning/intel/D1-CORE-PRODUCER-CONTRACT.md` | 126, 128 | Stale pinned digests | 🛑 Blocker | F-V1 |

**No debt-marker gate violations:** no unreferenced `TBD`, `FIXME` or `XXX` in any file this phase
created or modified.

---

## 12. Behavioural Spot-Checks

Run locally (Mac): source-truth checks only — no Cargo build or test run, per the standing rule.

| Behaviour | Check | Result | Status |
|---|---|---|---|
| Tool guard absent at the child registry seam | read `spawner.rs:4350-4366` | `build_tool_registry(&["Bash","Write"], IsolatedMutation, root, &[], rt)` — no parent-registry parameter exists | ✓ PASS (confirms the red) |
| `PolicyGate` unreachable | `grep -rn set_policy_gate crates/` | 2 hits: doc comment `engine.rs:2679`, definition `:4064`. Zero callers. 9 agent-path initialisers `None` | ✓ PASS (confirms the red) |
| F21-04-03 seam exists as cited | read `session_journal/reducer.rs:700-715` | the CAS is at :708-712, rejecting a mismatched `prior_cursor.journal_sequence` | ✓ PASS |
| Ratchet shipped | `git diff` `execution_policy.rs` | `else if matches!(source, PolicySource::Child) { stricter_approval_policy(...) }` + 2 tests | ✓ PASS |
| Contract digests after repair | recompute from `manifest.json` old vs new | `schema` same, `fixture` and `source_inputs` both moved | ✗ FAIL → F-V1 |
| `cargo fmt --all -- --check` | run | clean | ✓ PASS |
| Full suite | not run | Cargo is forbidden on this Mac; the phase's own Hetzner aggregate reads `11565/11565 passed, 48 skipped` at `f2d186f6` and is corroborated by the captured transcript | ? SKIP |

---

## 13. Deferred Items

None. Every gap above belongs to Phase 21's own criteria or to its own binding authorization
constraint; none is scheduled into Phase 22 or 23 by ROADMAP. The six HIGH findings *inherited* by
Phase 22 are consequences of Criterion 1 being unmet, not separately-scheduled work.

---

## 14. Gaps Summary

Phase 21 did honest, high-quality, genuinely live work and **correctly reports that it did not
achieve its goal.** Criterion 1 is not met, and I confirm both counterexamples at the cited lines.
That verdict should be trusted.

What must not be trusted without amendment is **Criterion 3's grade**. `MET WITH STATED
EXCEPTIONS` was awarded to an equivalence proof in which one of the two surfaces never got a
delegated child to take a single provider turn on either platform (F-V2), and in which seven of
eleven in-process pairings are the same function called twice (F-V3). The first defect is disclosed
inside `21-02-CORPUS-RESULTS.md` and then lost on the way to the verdict; the second is disclosed
only by implication. Both point the same way: the corpus proves less about surface equivalence than
its grade states, and the phase's own standard — *"a criterion that says any is not satisfied by
most"* — applied to Criterion 3 does not yield `MET-WITH-STATED-EXCEPTIONS`.

**One gap is closable inside this phase and should be closed before Phase 22 starts** (F-V1): the
repair moved `fixture_digest` as well as `source_inputs_digest`, `D1-CORE-PRODUCER-CONTRACT.md` §3
still pins the pre-repair values of both, and the panel's binding re-pin clause was declared
unsatisfiable after the wrong document was inspected. Two lines of intel and one corrected sentence.

**One gap materially weakens the phase's most-valued inheritance** (F-V4): the NO-CHANNEL canary
class the verdict calls *"worth more than any currently-green assertion in the phase"* is
fail-closed for one canary of three, conditionally fail-closed for a second, and never fails for
the third. If Phase 22 introduces the request channel the verdict predicts, only the provider
canary is guaranteed to notice.

Recommended before Phase 22 entry, in order: close F-V1 (mechanical); assert the budget canary and
close the approval canary's `Allowed`/`Allowed` hole (F-V4, small); amend the Criterion 3 grade and
`EVIDENCE-LIVE :: 1` with F-V2/F-V3 stated (documentation); correct
`21-04-ATTRIBUTION-RESULTS.md` §2 against its own ledger (F-V5); commit `.planning/TEST-AUDIT.md`
(F-V8). The six HIGH findings and the refund-across-restart question are Sean's to route.

---

_Verified: 2026-07-26T15:00:37Z at `1058965e`_
_Verifier: Claude (ferrox-verifier), adversarial goal-backward stance_
_Not committed — left to the orchestrator._
