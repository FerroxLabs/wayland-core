---
phase: 21-child-authority-and-budget-inheritance
plan: "03"
subsystem: child-authority
tags: [authority, approval-policy, tool-authority, policy-gate, contract-provenance]
status: complete
termination-state: "1 (complete) with two findings DECLINED and honestly open, and the one authorized repair recorded NOT-CLOSED on its live leg"
requires:
  - 21-01-ADMISSION-GATE.md
  - 21-01-AUTHORITY-CENSUS.md
  - 21-02-CORPUS-RESULTS.md
provides:
  - 21-03-REPAIR-SET.md
  - "child-sourced approval requests now ratchet instead of replacing"
  - "measured blast radius for all three HIGH candidate repairs"
affects:
  - crates/wcore-types/src/execution_policy.rs
  - crates/wcore-protocol/contracts/desktop/v1/
tech-stack:
  added: []
  patterns:
    - "approval posture is a ratchet for PolicySource::Child on BOTH branches, not only the managed one"
    - "SOURCE_INPUTS edits require a wcore-contract generate + check provenance re-pin"
key-files:
  created:
    - .planning/phases/21-child-authority-and-budget-inheritance/21-03-REPAIR-SET.md
    - .planning/phases/21-child-authority-and-budget-inheritance/21-03-t2-panel/
    - .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-03-t2-blastradius.log
    - .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-03-t3-linux.log
    - .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-03-t3-windows.log
  modified:
    - crates/wcore-types/src/execution_policy.rs
    - crates/wcore-protocol/contracts/desktop/v1/manifest.json
    - crates/wcore-protocol/contracts/desktop/v1/events/ready.json
    - crates/wcore-protocol/contracts/desktop/v1/adversarial/events/fixture-mismatch.jsonl
    - crates/wcore-protocol/contracts/desktop/v1/adversarial/events/schema-mismatch.jsonl
    - crates/wcore-protocol/contracts/desktop/v1/adversarial/events/version-mismatch.jsonl
    - .planning/BACKLOG.md
decisions:
  - "authorize-partial: repair F21-02-02 only; DECLINE F21-02-01 and F21-02-03 and leave both honestly open"
  - "F21-02-01 declined because the designed fix wires one of five production spawner sites and three of the other four have no parent ToolRegistry to intersect against"
  - "F21-02-03 declined because PolicyEngine is deny-by-default: wiring the gate on breaks 22 functional tests, and the useful form is the inheritance model census OOP-2 routed out of the phase"
  - "F21-02-02 recorded NOT-CLOSED on its live leg rather than claiming a closure no shipped surface can demonstrate"
metrics:
  repair-iterations-used: 1
  repair-iterations-permitted: 2
  findings-triaged: 10
  findings-authorized-fix: 1
  findings-declined: 2
  findings-backlogged: 7
  completed: 2026-07-26
---

# Phase 21 Plan 03: Child Authority Repair Set Summary

Measured the blast radius of all three HIGH corpus findings on real hardware,
authorized exactly one, and left two honestly open with their evidence.

## Termination state

**State 1 — complete**, with the important caveat that "complete" here means the
plan ran its full triage → authorization → repair → re-measure cycle and
terminated, NOT that Phase 21's Success Criterion 1 is closed. It is not. Two of
three HIGH findings are DECLINED and open, and the one repair that shipped is
recorded NOT-CLOSED on its live leg. **21-04 must state all three as explicit
exceptions.**

## Commits

| SHA | What |
|---|---|
| `b37db18f` | Task 1 — triage of every corpus red against the amended rules |
| `9cda3a22` | Task 2 — blast-radius probe, four-way panel, per-finding authorization |
| `e0cae85e` | Task 3 — the one authorized repair: child-sourced approval ratchet |
| `a412aba7` | Task 3 — mandated Desktop contract provenance re-pin |
| `7ece6c32` | Task 3 — post-repair delta and both platform transcripts |

Repaired tree SHA, asserted on both hosts before any build step:
`a412aba754b1bab5cd5764ed8e0d8502c9ee4020`.

## Triage

| Finding | Severity | Triaged | Authorized | Seam |
|---|---|---|---|---|
| F21-02-01 (tool / HIGH-1) | HIGH | FIX | **DECLINED** | `spawner.rs :: build_tool_registry` |
| F21-02-02 (approval / HIGH-4) | HIGH | FIX | **FIX** | `execution_policy.rs :: with_requested_approvals` |
| F21-02-03 (PolicyGate / HIGH-3) | HIGH | FIX | **DECLINED** | `engine.rs :: policy_gate` |
| F21-02-04 … F21-02-10 | MEDIUM ×4, LOW ×3 | BACKLOG | BACKLOG | — |

The corpus measured **zero CRITICAL**. HIGH-2 (provider) carries no corpus
finding row — the corpus recorded the dimension NO-CHANNEL on all eight
combinations and added a live structural canary instead — so it is in the
admission gate's scope ceiling but was never flagged, and is recorded as such
rather than silently dropped.

## Two things the triage found that the plan did not anticipate

**1. The admission gate's binding contract constraint fires on ALL THREE HIGH
findings, not just HIGH-4.** `contract/spec.rs:833-874` lists 40 generator
source inputs whose *contents* are fingerprinted. `bootstrap.rs`,
`execution_policy.rs` and `engine.rs` are all members, so every candidate repair
moves `source_inputs_digest`. Measured, not assumed — the probe recomputed the
digest after each candidate and all three moved it.

**2. That cost is far smaller than the gate's wording implies, and the gate's D1
clause is unsatisfiable as written.** `schema_digest` did NOT move for any
candidate; only the source-provenance fingerprint did. `git log` over the
contract corpus shows 16 regenerations, three of them titled verbatim
`chore(protocol): re-pin Desktop contract provenance digests`, and commit
`b6936299`'s message documents this exact situation and its fix. Meanwhile the
gate requires "re-pinning D1 section 3" — and
`.planning/intel/DESKTOP-PROTOCOL-CHECKPOINT.md` is four paragraphs long, has no
section 3, and pins no digests at all. The satisfiable parts (`digest`, `check`,
record) were done; the D1 clause names something that does not exist.

> ### CORRECTION (2026-07-26, post-verification) — both claims above are wrong
>
> Phase 21 verification raised this as finding **F-V1 (HIGH)**
> (`VERIFICATION.md` §6). Both halves of item 2 are retracted:
>
> 1. **"only the source-provenance fingerprint" moved is false.** `fixture_digest`
>    moved too — `42f142ab…` → `0704cd43…` at `a412aba7`. The move is mechanical
>    (the descriptor is embedded in `events/ready.json` and three adversarial
>    negotiation vectors, all of which are inside the 151-file fixture set), but
>    `fixture_digest` is the digest that gets its own named negotiation failure
>    (`FixtureDigestMismatch`) and the one a Desktop consumer/reducer conformance
>    harness replays against. Its move was disclosed nowhere in this phase.
> 2. **"the D1 clause names something that does not exist" is false — wrong
>    document was inspected.** The gate's "D1 §3" means
>    `.planning/intel/D1-CORE-PRODUCER-CONTRACT.md`, which has a `## 3. Digests`
>    section pinning all three values. The clause was satisfiable all along.
>
> **Discharged 2026-07-26.** `D1-CORE-PRODUCER-CONTRACT.md` is now at revision 2:
> pinned SHA `a412aba7`, both moved digests re-pinned, and a new §3.0 publishes the
> move as an explicit contract bump with before/after values, cause commit, and
> Desktop impact. Re-proved on `hetzner-dsm:/root/wayland-p21` with the canonical
> Rust tool — `wcore-contract digest` EXIT=0 and `wcore-contract check` EXIT=0
> (*"Desktop contract corpus is current"*), plus `cargo nextest run -p
> wcore-protocol --no-fail-fast` 302/302. The `check` leg is the load-bearing one:
> it re-derives the corpus from the generator sources, so the corpus is proved
> neither stale nor hand-edited.

## Blast radius — measured, not described

Each candidate applied alone in a throwaway Hetzner worktree created from the
triage commit and **deleted afterwards**, reverted between candidates, verdicts
emitted by the script itself. Baseline 3421 passing over the five crates the
seams touch.

| Finding | Verdict | Before | After | Verdict-changing tests |
|---|---|---|---|---|
| F21-02-01 | WIDE | 3421 | 3420 | **1** — the contract provenance pin only |
| F21-02-02 | WIDE | 3421 | 3420 | **1** — the contract provenance pin only |
| F21-02-03 | WIDE | 3421 | 3398 | **23** — the pin plus **22 functional tests** |

Nothing returned BREAKS-BUILD. F21-02-03's 22 functional failures span
`dangerous_lease_e2e_test`, `engine::audit_2026_05_22_tests`,
`execution_posture_e2e_test`, `json_stream_approval_test`,
`output_compaction_test`, `runaway_loop_test`,
`typed_execution_policy_e2e_test` and `w9_1_skill_drafting_per_turn` — the
deny-by-default catastrophe arriving as a number.

A methodological note against myself: the probe's first-pass inline extraction
missed nextest's `TRY n TMT/FAIL [` prefix and the digest binary's `key=value`
output, so its verdict lines were computed against an empty non-passing
baseline. A corrected second pass re-derived every verdict from the same run's
retained artifacts; both passes are in the transcript, with the superseded lines
renamed so exactly one authoritative verdict line per finding survives.

## The authorization

`PANEL-DECISION :: authorize-partial :: MAJORITY` — 3-1 over the adversarial
member's `disprove-and-correct`. `codex-sol`, `gemini-pro`, `kimi-k3` all chose
`authorize-partial`; the internal adversarial pass argued the case nobody else
would, that the system was right and the corpus asked an unaskable question, and
lost on two grounds it recorded itself.

**The set-level vote was not the interesting part; the per-finding split was.**
`gemini-pro` and `kimi-k3` authorized both F21-02-01 and F21-02-02. `codex-sol`
dissented, authorizing only F21-02-02, on the ground that wiring
`parent_tool_authority` at `bootstrap.rs` leaves other production spawner sites
fail-open. **That claim was re-verified against the live tree rather than taken
on the panel's word, and it is true:**

- `crates/wcore-cli/src/workflow.rs:173` — no `#[cfg(test)]` anywhere above it
- `crates/wcore-cli/src/crucible.rs:36` — same
- `crates/wcore-agent/src/engine.rs:12061` — production transient spawner
- `crates/wcore-agent/src/orchestration/anvil/seat.rs:91` — production

At all four, `parent_tool_authority` would stay `None` and the intersection
would be skipped. Worse, three of the four construct their spawner from a
`Config` with **no parent `ToolRegistry` in scope at all**, so there is nothing
there to intersect against without making parent tool authority a first-class
concept across the spawner API — an architecture change, not a wiring line.

So the per-finding outcome followed the verified evidence rather than the
headcount. Shipping the designed fix would have put a fail-open guard at one
caller while four production routes bypassed it — the exact shape the plan's
"repair at the seam, not wherever it is easiest" rule forbids, and the same
fail-open shape being declined one finding later in F21-02-03.

### Independence defect, recorded because concealing it would be worse

The four captures were written into the members' working directory.
`gemini-pro` touched no sibling file. `kimi-k3` noticed they existed and
explicitly declined to read them. **`codex-sol` ran a ripgrep across the parent
directory that surfaced `gemini-pro.raw.txt:127` — gemini's position paragraph —
and `21-03-PLAN.md` including this task's own gate scripts.** Its set-level vote
is therefore not independent and is recorded as such. Bounding the damage:
codex reached a *different* per-finding conclusion, and the fact its dissent
turns on was independently re-verified against source before being acted on.
Discarding codex's vote entirely, `authorize-partial` still leads 2-1. Fix for
any future panel: give each member its own working directory.

## What shipped

**One production change.** `with_requested_approvals` now ratchets a
`PolicySource::Child` request on the non-managed branch, so a child-sourced
`Bypass` can no longer replace a `Prompt` parent. Scoped to `Child`; a second
test pins that every other source still selects its requested posture. Plus the
mandated provenance re-pin of five contract files.

The property was not broken in the shipped product — `PolicySource::Child` has
no production constructor, which is why the corpus recorded the approval
dimension NO-CHANNEL rather than ALLOWED. **That is the point.** It held by the
absence of a request channel, not by enforcement. It now holds by enforcement.

## The delta, stated against what 21-02 measured

The corpus harness's own resolver observation, same extraction over both
transcripts:

```
21-02 (before) :: posture Smart, approvals Bypass, source Child, managed false
21-03 (after)  :: posture Smart, approvals Prompt, source Child, managed false
```

All eleven cases otherwise hold exactly the outcome 21-02 measured, on both
platforms, in all four combinations. Zero regressions, zero new widenings.

| Gate | Result |
|---|---|
| Hetzner aggregate | 11545 run, **11545 passed** (1 slow, 2 flaky), 48 skipped — was 11543 passed; the +2 are the new ratchet tests |
| Workspace clippy `--all-targets -D warnings` | clean |
| Corpus suite, Linux | 23/23 |
| Corpus suite, Windows | 19/19, with clippy clean **first** |
| `wcore-types`, Windows | 132/132, including both new ratchet tests |
| Contract corpus + adversarial, Windows | 32/32 |
| Repaired binary | built and executed on both platforms |

The shipped binary was observed emitting its real `ready` frame carrying
`"source_inputs_digest":"sha256:9d5928b4…"` and the **unchanged**
`"schema_digest":"sha256:e5d1744a…"` — live proof that the re-pin is provenance,
not wire.

> **CORRECTION (2026-07-26).** This paragraph cites two of the three digests and
> omits the one that also moved. The same `ready` frames carry
> `"fixture_digest":"sha256:0704cd43…"`, up from `sha256:42f142ab…`. Counted over
> the transcripts: `21-03-t3-linux.log` has 11 occurrences of each of the three
> post-repair values and zero of the pre-repair ones; `21-03-t3-windows.log` the
> same; `21-02-t3-linux.log` (pre-repair) has 11 × `42f142ab…` and 11 ×
> `d8b1a8b5…`. So *"the re-pin is provenance, not wire"* is right about
> `schema_digest` and wrong as stated: `fixture_digest` is a negotiated wire field
> with its own fail-closed error, and it moved. See
> `.planning/intel/D1-CORE-PRODUCER-CONTRACT.md` §3.0 and §3.2 leg C.

## Closure — the honest verdict

```
CLOSURE :: F21-02-02 :: NOT-CLOSED :: green :: red
```

The repair is correct and proved by the harness's own observation, but its live
leg is not green and cannot be: the approval dimension is `NOT-EXPRESSIBLE` on
every live combination because **no shipped surface offers a child any way to
request an approval posture**. The gate's vocabulary has only `green`/`red`, and
`NOT-EXPRESSIBLE` is recorded **red** deliberately — it was not observed
failing, it could not be observed at all, and recording it green would claim a
live closure this plan did not earn. That is precisely what the closure rule
exists to prevent.

## Findings left OPEN by decision

- **F21-02-01 (HIGH, tool authority).** Confirmed by the product's own unit test
  at `spawner.rs:4357`: `build_tool_registry(&["Bash","Write"], IsolatedMutation, …)`
  registers Bash without ever consulting a parent. Declined because no repair
  available inside this plan closes it. **Success Criterion 1 cannot be claimed
  for the tool dimension.**
- **F21-02-03 (HIGH, PolicyGate reachability).** Confirmed unreachable: zero
  callers of `set_policy_gate`, every agent-path initialiser `None`. Declined at
  a measured cost of 22 functional tests plus an architecture change census
  OOP-2 already routed out of the phase.
- **F21-02-02's live closure**, per above.

## Residuals, named and not annotated away

Two flaky-then-passing tests in the aggregate, neither targeted here:
`packaged_core_cancels_an_active_stream` (FLAKY 3/3 — already corpus finding
F21-02-10 and `TEST-AUDIT.md:171`) and `harness_tui_flow
agent_turn_streams_mock_assistant_text_into_the_transcript` (FLAKY 2/3, passed
on retry, **not** in the 21-02 measurement). The second is a new observation. It
did not fail the run, it is in no file this plan touched, and it is routed to
BACKLOG rather than repaired — repairing an untargeted flake is the
unbounded-scope move the phase rules forbid.

No test was newly ignored (47 → 47) and lint suppressions did not grow
(237 → 237), both measured against the phase base `dd02a624`.

## Recorded unknowns

- Whether a real adversary can reach F21-02-01's widening, or only a cooperating
  caller. The live surfaces could not express it in a hermetic non-repository
  workspace; a real git-repository fixture was not built inside the cap.
- Whether the provenance bump has downstream effects the Desktop conformance
  harness would catch. CTRL-02/D1's consumer half is in another repository.
- Whether declining F21-02-01 leaves the tool dimension exposed in practice,
  given the isolated-worktree mitigation refuses in every workspace the corpus
  could construct.

## Iterations

**One of the two permitted.** A single edit-build-run cycle produced a clean
ordered gate on both platforms.

No requirement is marked complete — closure is 21-04's to claim, and on this
evidence it must claim it with three named exceptions.

## Self-Check: PASSED

All ten declared artifacts exist on disk and all five declared commits resolve
in this repository.
