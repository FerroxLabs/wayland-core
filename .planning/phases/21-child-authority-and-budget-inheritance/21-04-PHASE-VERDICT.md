# Phase 21 — Verdict

**Phase**: 21, Child Authority and Budget Inheritance
**Goal (ROADMAP.md:75)**: Every delegated actor remains inside the parent's
authority and resource envelope.

BASE-SHA :: f2d186f6c3e77b99961171632fbbfce5c5b5d776

SEAL :: NOT-CLAIMED

**No seal is claimed and no candidate is prepared.** Sealing is a Sean action
and always has been. This document states what the evidence supports and, at
least as importantly, what it does not.

**Scope ceiling, carried from `21-01-ADMISSION-GATE.md:173`.** This is a CORE
verdict against the standalone surface and the Core producer contract as pinned.
Reserved until CTRL-02 closes: any whole-Wayland claim, any statement about
Desktop consumer or reducer behaviour, and any assertion that D1 section 9 items
1 and 2 are discharged. Nothing below makes one.

---

## 1. The three Success Criteria, graded

The criterion text below is copied verbatim from `ROADMAP.md`. It is the only
completion authority. The temptation this document is most exposed to is
narrowing a criterion until the evidence in hand satisfies it; `MET WITH STATED
EXCEPTIONS` exists as a first-class verdict precisely so there is no incentive
to do that. The criteria are fixed; the verdict moves.

CRITERION :: 1 :: NOT-MET :: A child cannot widen any provider, tool, filesystem, egress, secret, approval, depth, fan-out, time, token, or cost restriction.
CRITERION :: 2 :: MET-WITH-STATED-EXCEPTIONS :: Nested reservation, refund, escalation, approval, cancellation, and result delivery remain attributable to the correct parent/session.
CRITERION :: 3 :: NOT-MET :: Standalone and host-protocol hostile corpora prove equivalent enforcement.

AMENDED :: 2026-07-26 :: CRITERION 3 :: MET-WITH-STATED-EXCEPTIONS -> NOT-MET :: at 359ce2bf, per VERIFICATION.md F-V2/F-V3 and the repair recorded in 21-05-CRITERION3-REPAIR.md

EVIDENCE-LIVE :: 1 :: .planning/phases/21-child-authority-and-budget-inheritance/21-02-CORPUS-RESULTS.md :: eleven dimensions driven on both platforms in four surface/mode combinations; filesystem, egress and secret REFUSED live on Linux and Windows; provider NO-CHANNEL; tool, approval, depth, fan-out, time, token and cost NOT-EXPRESSIBLE on at least one live combination -- SUPERSEDED, see EVIDENCE-LIVE :: 4: every decisive STANDALONE live verdict in this row came from a run with zero child provider turns
EVIDENCE-LIVE :: 4 :: .planning/phases/21-child-authority-and-budget-inheritance/21-05-CRITERION3-REPAIR.md :: at 359ce2bf, all fourteen decisive live rows on Linux carry one or two delegated child provider turns on BOTH surfaces; tool, filesystem, secret, egress and depth REFUSED live with a real actor; provider and approval NO-CHANNEL with a real actor; fan-out and time/token/cost NOT-EXPRESSIBLE live
EVIDENCE-LIVE :: 1 :: .planning/phases/21-child-authority-and-budget-inheritance/21-03-REPAIR-SET.md :: the one authorized repair recorded CLOSURE :: F21-02-02 :: NOT-CLOSED on its live leg, because no shipped surface offers a child any way to request an approval posture
EVIDENCE-LIVE :: 2 :: .planning/phases/21-child-authority-and-budget-inheritance/21-04-ATTRIBUTION-RESULTS.md :: six lifecycle events, two siblings each, three generations for escalation and delivery; five of six CORRECT at the real in-process seam on both platforms; zero MISATTRIBUTED anywhere; per-sibling parent_call_id observed on the real wire in every run where the sibling pair survived
EVIDENCE-LIVE :: 3 :: .planning/phases/21-child-authority-and-budget-inheritance/21-02-CORPUS-RESULTS.md :: the standalone and host-protocol surfaces reached the same enforcement verdict on every decisive dimension, asserted structurally by assert_surface_equivalence rather than compared by eye -- QUALIFIED: for 7 of 11 dimensions the two drivers called the SAME function, so the assertion could not have failed on them; see 21-05-CRITERION3-REPAIR.md section 2
EVIDENCE-LIVE :: 5 :: .planning/phases/21-child-authority-and-budget-inheritance/21-05-CRITERION3-REPAIR.md :: at 359ce2bf the host-protocol driver reaches 9 of 11 dimensions through the production AgentBootstrap + HostChildController path; the remaining 2 record NOT-EXPRESSIBLE with the SubAgentConfig field set as evidence, read by exhaustive destructuring so a new child-request field cannot reach the product without the record being revisited

### Criterion 1 — NOT MET

The word the criterion turns on is **any**. Eleven restrictions are named and
the claim is that a child cannot widen ANY of them. Two are open by Sean's own
authorization and cannot be claimed:

* **tool** — `21-03-REPAIR-SET.md` records F21-02-01 DECLINED and open, confirmed
  by the product's own unit test at `spawner.rs:4357`:
  `build_tool_registry(&["Bash","Write"], IsolatedMutation, …)` registers Bash
  without ever consulting a parent. 21-03's own summary states it plainly:
  *"Success Criterion 1 cannot be claimed for the tool dimension."*
* **approval** — F21-02-02 was repaired (a child-sourced `PolicySource::Child`
  request now ratchets instead of replacing) but its closure is recorded
  `NOT-CLOSED` on the live leg, because no shipped surface offers a child any way
  to request an approval posture at all. The property holds by enforcement in
  process and by absence of a channel in the product.

A third finding, F21-02-03 (PolicyGate unreachable — zero callers of
`set_policy_gate`, every agent-path initialiser `None`), is DECLINED and open at
a measured cost of 22 functional tests plus an architecture change the census
already routed out of phase.

Six of the eleven dimensions (provider, approval, depth, time, token, cost —
plus tool and fan-out on at least one platform) were recorded `NO-CHANNEL` or
`NOT-EXPRESSIBLE` live rather than `REFUSED`, meaning the property currently
holds in part **by the absence of a request channel rather than by enforcement**.
Three (filesystem, egress, secret) were driven live on both platforms and
refused. That is real and it is not nothing. It is also not eleven.

**Grading this MET, or even MET-WITH-EXCEPTIONS, would be the narrowing this
plan was written to avoid.** A criterion that says *any* is not satisfied by
*most*.

**Cross-audited, because this is the phase's most consequential call.**
`21-04-t3-panel/PANEL.md` records the four-way panel: `codex-sol`,
`gemini-3.1-pro` and `kimi-k3` each returned `NOT-MET` independently, and the
internal adversarial pass argued the strongest available case for
`MET-WITH-STATED-EXCEPTIONS` — that the middle verdict becomes unreachable if any
open exception forces failure, that no amplification was ever OBSERVED, and that
Criteria 2 and 3 carry named gaps too. It lost on one point that is checkable
from `ROADMAP.md:79` without the panel: Criterion 1 is the only one of the three
that carries a universal over an ENUMERATED list, and its enumeration contains a
member the product's own unit test falsifies. Criteria 2 and 3 have gaps in
PROOF; Criterion 1 has a guard confirmed absent. The panel's unanimity is
discounted in the record because all three members received the same framing.

### Criterion 2 — MET WITH STATED EXCEPTIONS

Measured in this plan. Six lifecycle events, every case with at least two
siblings and three generations where the question is which ancestor an event
rolls up to.

| Event | In-process, both platforms | Live |
|---|---|---|
| reservation | **CORRECT** | not observable on the wire |
| refund (crash + restart) | NOT-OBSERVABLE — see exception 2 | not observable on the wire |
| escalation | **CORRECT** | not observable on the wire |
| approval | **CORRECT** | frames arrive, name no sibling |
| cancellation | **CORRECT** | no per-child command exists |
| result delivery | **CORRECT** | **observed correct on the real wire** |

**Zero MISATTRIBUTED verdicts were measured, anywhere, in any mode, on either
platform.** Nothing in this corpus caught the product putting a nested event on
the wrong actor. And the in-process-pass-against-live-misattribution class this
plan was required to hunt for has zero members: where the product can be
observed at all, the plumbing and the product agree. Both are genuine positive
results.

The live half of the claim rests on one solid observation and three gaps:

* **Solid.** Every run in which the sibling pair survived produced exactly two
  distinct `parent_call_id` values on the shipped `--json-stream` wire, with each
  sibling's own result under exactly one of them and never under the other. That
  is result-delivery attribution proved on the real binary, not inferred.
* **Exception 1 (F21-04-01, HIGH).** The host protocol carries no per-child
  observable for reservation, refund, escalation or cancellation, and no field on
  `approval_required` naming the sibling that raised it. Five of the criterion's
  six events are therefore proved at the in-process seam ONLY. A host driving
  Core over the protocol cannot render, address or audit them per child. This is
  an observability gap, not a demonstrated misattribution.
* **Exception 2 (F21-04-02, HIGH).** A provider reservation handle does not
  survive a process restart. After dropping and rebinding the coordinator over
  the same journal, both siblings' reserved totals read zero and the release
  returned false. Whether the gap is in this corpus's binding of the durable path
  or in the product is not settled — the two permitted harness iterations are
  spent and a third is forbidden. The refund leg of Criterion 2 is therefore
  UNPROVEN across a crash, which is the only condition under which durable budget
  authority is worth anything.
* **Exception 3 (windows-tui, MEDIUM).** Approval and cancellation as a HUMAN
  sees them are unprovable in the TUI on Windows, because
  `pty_capture.rs` is `#![cfg(unix)]` and `support/pty.rs` inherits the gate. No
  headless or json-stream result was substituted for the missing evidence.

### Criterion 3 — NOT MET (amended 2026-07-26; was MET WITH STATED EXCEPTIONS)

**This grade is withdrawn and replaced.** It was awarded to a proof that, as
written at `1058965e`, could not have failed. Verification found two independent
defects (`VERIFICATION.md` §4, §5) and both were confirmed by measurement rather
than argued:

1. **The standalone half of the equivalence had no actor.** Not one standalone
   LIVE run on either platform got a delegated child to a provider turn. All
   twelve decisive standalone live `REFUSED` verdicts came from runs recording
   `child_turns=0`. The cause is a product fact:
   `wcore_agent::confirm::ToolConfirmer::check_for` denies any tool call needing
   confirmation when stdin is not a terminal, so the piped headless transport
   refused the `Delegate` call before any child existed. The corpus's own
   transcripts say `X Tool execution denied by user`. The refusals were absences
   of effect from an actor that never acted.
2. **Seven of eleven in-process pairings were one function called twice.** The
   host-protocol driver dispatched to the standalone driver's probe functions, so
   `assert_surface_equivalence` could not fail on them.

Together, the `MET WITH STATED EXCEPTIONS` grade rested on two identical
in-process calls agreeing with each other, one genuine two-seam in-process
comparison, and a live comparison between a surface with a child and a surface
without one. The section above described the *first* exception (what the set was)
and missed the *load-bearing* one (that half of it had no actor). That was the
error, and it is this document's error, not the verifier's.

#### What equivalence is proved OVER at `46dd076a`

The repair is recorded in `21-05-CRITERION3-REPAIR.md`. Restated as the clause
this criterion is now graded against:

> Across the standalone and host-protocol surfaces, driven both in process and
> against the real `wayland-core` binary, a delegated actor that **demonstrably
> took its own provider turn** did not obtain a filesystem root outside its
> parent's, a credential file its parent's policy denies, an outbound
> destination its parent's policy does not permit, nesting depth beyond its
> parent's seeded envelope, a Bash effect its read-only parent does not hold, a
> provider its parent does not hold, or an approval posture weaker than its
> parent's — on **Linux**.

That clause is now true, checked, and non-vacuous: all fourteen decisive
live rows carry one or two child provider turns, and the two surfaces are driven
through genuinely different object graphs (production `AgentBootstrap` +
`HostChildController` against a bare `AgentSpawner` + `spawn_fork`).

#### Why that clause is still not the criterion

The criterion says *standalone and host-protocol hostile corpora prove EQUIVALENT
enforcement*, without qualification. Four unmet clauses:

* **Three of eleven dimensions have no host-protocol expression at all.** Tool
  and fan-out cannot be requested on the host child-spawn API; egress cannot be
  attempted because the child registry carries no network-capable tool
  (`Unknown tool: WebFetch`, measured). Equivalence is not established over
  those three; their inexpressibility is recorded, not equated.
* **Fan-out is undetermined live**, on both platforms and both surfaces.
* **The Windows standalone live surface has no actor at all.** Every PTY-backed
  transport is unavailable there, and the piped fallback has no approval channel,
  so no delegated child can act. Windows equivalence is therefore proved over
  the in-process modes and the host-protocol live mode only.
* **The tool dimension's REFUSED is jointly attributable** to tool authority and
  to workspace containment, and Criterion 1 records the tool guard as ABSENT — so
  that REFUSED must not be read as evidence of tool enforcement.

The phase's own standard — *a criterion that says ANY is not satisfied by MOST* —
applied honestly to Criterion 3 yields NOT-MET. Recording it that way costs this
phase its second-best grade and is the correct call: the previous grade's only
support was a comparison that could not have come out any other way.

---

## 2. Fan-out

FANOUT :: DISTINCT-AND-COVERED :: Fan-out is genuinely distinct from concurrency - they are different mechanisms with different numbers - but the resource envelope fan-out could amplify is already bounded, so no new knob is added and no max_fan_out is designed.

Restated verbatim from `21-01-AUTHORITY-CENSUS.md:162` so F21-02's mention of
fan-out is answered explicitly rather than left implicit in a census nobody
re-reads. 21-02 recorded the fan-out dimension `NOT-EXPRESSIBLE` live on both
platforms.

---

## 3. Requirements

REQUIREMENT :: F21-01 :: OPEN :: The tool dimension of the intersection is confirmed absent (`build_tool_registry` registers a requested tool without consulting the parent) and F21-02-01 is DECLINED and open at Sean's authorization; provider intersection has no request channel to intersect. Marking this complete would claim an intersection the product does not compute.
REQUIREMENT :: F21-02 :: OPEN :: Depth, time, token and cost refuse at the in-process ancestor-rollup seam, but all four were NOT-EXPRESSIBLE on both live combinations because no shipped surface carries a child-fillable budget field. The property holds in part by absence of a request channel, and a requirement is never marked complete on in-process evidence alone.
REQUIREMENT :: F21-03 :: OPEN :: Five of six lifecycle events attribute correctly at the real seam on both platforms with zero misattributions, and result delivery is proved correct live on the shipped wire. Refund across a crash is UNPROVEN (F21-04-02) and four of six events have no per-child observable on the host protocol at all (F21-04-01), so the requirement's "remain attributable" cannot be asserted for the whole set.
REQUIREMENT :: F21-04 :: OPEN :: The hostile corpora ran on both surfaces and both platforms and found no amplification on any dimension they could express, but tool authority stays confirmed-absent and DECLINED, and F21-04-03 shows two parallel siblings failing outright on the shipped binary — so "hostile child tests prove no amplification" is not yet a claim the evidence carries end to end.

**All four requirements are left OPEN.** Not one is marked complete. Three of
the four are open on live-evidence grounds rather than on in-process failure,
which is the distinction this codebase learned the hard way when an entire
permission crate passed its own tests while no consumer called it.

---

## 4. Open findings routed to Sean

CRITICAL or HIGH must be fixed or disproved. None was fixed here — this plan
repairs nothing, and 21-03's repair budget is spent with a third cycle forbidden.

| Finding | Severity | From | State |
|---|---|---|---|
| F21-02-01 — tool authority is not intersected at the spawn seam | HIGH | 21-02 / 21-03 | DECLINED by authorization, open |
| F21-02-03 — `PolicyGate` unreachable, zero callers, fail-open | HIGH | 21-02 / 21-03 | DECLINED by authorization, open |
| F21-02-02 — child-sourced approval ratchet | HIGH | 21-03 | repaired; live closure NOT-CLOSED |
| F21-04-01 — no per-child observable on the host protocol for four of six lifecycle events, and no sibling identity on `approval_required` | HIGH | 21-04 | open, new |
| F21-04-02 — a provider reservation handle does not survive a restart, so a refund cannot be attributed across a crash | HIGH | 21-04 | open, new, cause not isolated |
| F21-04-03 — two parallel `Spawn` siblings fail outright with a journal-head CAS collision; the losing sibling's budget authority is left PERMANENTLY FAULTED and the session carries a nonterminal tool execution | HIGH | 21-04 | open, new |

MEDIUM and below are logged to `.planning/BACKLOG.md` and do not block:
F21-04-04 (`ParentTurn` delivery unexercised), F21-04-05 (`charge` does not block
a session), F21-04-06 (`stream_end` is per stream, not per turn), and the
windows-tui limitation.

### On F21-04-03 specifically

It is worth separating from the rest because it is not an attribution defect at
all — it is a **parallel-delegation defect on the product's advertised fan-out
path**, found because an attribution corpus is the first thing in this phase to
run two siblings at once against the shipped binary.

* Measured on Linux in 3 of the 8 live runs in the recorded transcript, and on
  Windows in **6 of 6** json-stream runs. It is a race on Linux and effectively
  deterministic on Windows at the recorded SHA.
* The seam is `session_journal/reducer.rs:708`, which rejects a budget-authority
  append whose `prior_cursor.journal_sequence` no longer matches `state.last_seq`.
  Two concurrent siblings each capture the journal head; the second loses.
* The loser does not retry. Its authority is reported *permanently faulted*, both
  siblings die, and the parent session is left with `turn … has nonterminal tool
  execution`.
* Checked and NOT a known red. The string appears nowhere in the Phase 20/20A
  handoff, `BACKLOG.md`, `21-02-CORPUS-RESULTS.md` or `21-03-REPAIR-SET.md`, and
  it is distinct from 21-02's F21-02-08 (a missing ephemeral vault under a
  hermetic home — the vault is configured here and the session starts cleanly).

---

## 5. What phases 22 and 23 inherit

Both are serial behind this one, so the gaps below are theirs whether or not
they are read.

1. **Three HIGH findings open by authorization** (F21-02-01, F21-02-03, and
   F21-02-02's live closure) and **three new HIGH findings** (F21-04-01,
   F21-04-02, F21-04-03). Six HIGH items enter Phase 22 open.

2. **Dimensions whose property holds by ABSENCE OF A REQUEST CHANNEL rather than
   by enforcement**: provider, approval, and the `Some(..)` legs of depth, time,
   token and cost. This is the single most important inheritance in this document.
   **Phase 22's supervision and durable-goal work is exactly where such a channel
   is most likely to appear**, and the day one does, an unenforced dimension stops
   being theoretical. 21-02's corpus carries NO-CHANNEL canaries built to go red
   on that day; they are worth more than any currently-green assertion in the
   phase, and Phase 22 must not weaken or delete them.

   **Correction, 2026-07-26 (VERIFICATION.md F-V4).** As shipped at `1058965e`
   these canaries could NOT go red on that day. The budget canary returned a
   `String` that nothing asserted on. The approval canary was excused by an early
   return on its VACUOUS census verdict, so in the exact scenario it exists for —
   a channel appears AND is live-exploitable — every assertion in the harness
   passed. The paragraph above was the most important claim in this document and
   it was false. Repaired at `46dd076a`
   (`assert_no_channel_canaries_stayed_intact`, checked BEFORE the equivalence
   pair, on two independent triggers and independent of census verdict). Proved
   by injecting a production file naming the child-sourced policy request type
   into the real tree: the suite fails `NO-CHANNEL CANARY TRIPPED` and goes green
   again the moment that file is removed. Four permanent tests pin the same
   behaviour as data, including one that pins that every OTHER assertion stays
   green on that scenario, so the canary can never again be mistaken for
   redundant. See `21-05-CRITERION3-REPAIR.md` section 3.

3. **F21-04-03 blocks parallel supervision work directly.** Phase 22 supervises
   fleets of children. Two siblings already collide on the journal head about half
   the time on Linux and every time on Windows, and the loser's authority is
   permanently faulted rather than retried. Any Phase 22 fan-out proof will hit
   this before it hits anything of its own.

4. **The host protocol cannot address or audit an individual child.**
   `ProtocolCommand` has only a whole-turn `Stop`. A supervision contract that
   promises per-child pause, cancel or inspection over the protocol needs a wire
   surface that does not exist yet.

5. **The Windows TUI stays undrivable** until `portable_pty`'s ConPTY backend
   surfaces the child's stdout. Every human-visible property is provable on Linux
   and macOS only.

6. **A maintenance cost worth naming**: the rendered-screen driver asserts against
   painted text. That is the right surface for proving a human sees the right
   thing, and it will break when the UI changes. The alternative — asserting only
   on the wire — is what lets a product ship with correct plumbing and a wrong
   screen, so the cost is worth paying deliberately rather than discovered later.

---

## 6. Termination

This plan ended in **state 2 — complete, criteria met with stated exceptions**.
The attribution corpus was authored once, executed once per platform, and one
verdict was stated. No fifth Phase 21 plan was created or proposed. No production
file under `crates/*/src` was touched. No existing test was modified, renamed,
re-gated, `#[ignore]`d or deleted. No production observability hook was added to
make a test possible. Nothing was repaired. No seal is claimed.
