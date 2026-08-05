---
phase: 21-child-authority-and-budget-inheritance
plan: "04"
subsystem: child-attribution
tags: [attribution, delegation, budget-authority, durable-child, approval, host-protocol]
status: complete
termination-state: "2 (complete — criteria met with stated exceptions). Criterion 1 NOT MET, Criteria 2 and 3 MET WITH STATED EXCEPTIONS, all four requirements left explicitly OPEN, no seal claimed."
requires:
  - 21-01-ADMISSION-GATE.md
  - 21-01-AUTHORITY-CENSUS.md
  - 21-02-CORPUS-RESULTS.md
  - 21-03-REPAIR-SET.md
provides:
  - 21-04-ATTRIBUTION-RESULTS.md
  - 21-04-PHASE-VERDICT.md
  - "a six-event attribution corpus, two siblings per case, in process and live on the real binary"
affects:
  - crates/wcore-cli/tests/child_attribution_corpus.rs
  - crates/wcore-cli/tests/child_attribution_corpus/
tech-stack:
  added: []
  patterns:
    - "every attribution case runs at least two siblings, asserted structurally, so a misattribution has somewhere wrong to land"
    - "per-sibling `parent_call_id` on the shipped --json-stream wire is the live attribution observable"
    - "an unobservable event is recorded NOT-OBSERVABLE, never asserted weakly and never given a production hook"
key-files:
  created:
    - crates/wcore-cli/tests/child_attribution_corpus.rs
    - crates/wcore-cli/tests/child_attribution_corpus/cases.rs
    - crates/wcore-cli/tests/child_attribution_corpus/live.rs
    - .planning/phases/21-child-authority-and-budget-inheritance/21-04-ATTRIBUTION-RESULTS.md
    - .planning/phases/21-child-authority-and-budget-inheritance/21-04-PHASE-VERDICT.md
    - .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t1-linux-suite.log
    - .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
    - .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-windows.log
  modified:
    - .planning/REQUIREMENTS.md
    - .planning/BACKLOG.md
decisions:
  - "grade Criterion 1 NOT MET rather than MET-WITH-EXCEPTIONS: the criterion says a child cannot widen ANY of eleven restrictions, and two are open by Sean's own authorization"
  - "leave all four requirements OPEN; never mark one complete on in-process evidence alone"
  - "report F21-04-03 (parallel siblings die on a journal-head CAS collision) as a new HIGH after checking it against 21-03's open findings and the Phase 20/20A known-red list"
  - "record refund-across-restart NOT-OBSERVABLE and escalate rather than guess whether the cause is the harness or the product — the two permitted harness iterations were spent"
  - "use Spawn rather than Delegate for the live driver: only SpawnTool mints a per-task parent_call_id, so only that path puts sibling attribution on the wire"
metrics:
  lifecycle-events-covered: 6
  siblings-per-case: 2
  in-process-correct: 5
  misattributions-measured: 0
  harness-repair-iterations-used: 2
  harness-repair-iterations-permitted: 2
  new-findings: 6
  new-high-findings: 3
  completed: 2026-07-26
---

# Phase 21 Plan 04: Child Attribution and Phase Verdict Summary

Six nested lifecycle events proved attributable at the real seams with zero
misattributions anywhere, three new HIGH findings surfaced by running two
siblings at once against the shipped binary, and Phase 21 graded honestly: one
criterion NOT MET, two MET WITH STATED EXCEPTIONS, all four requirements OPEN,
no seal.

## Termination state

**State 2 — complete, criteria met with stated exceptions.** The attribution
corpus was authored once, executed once per platform, and one verdict was
stated. No fifth Phase 21 plan was created or proposed.

## Run identity

Corpus SHA, asserted on both hosts before any build step:
`f2d186f6c3e77b99961171632fbbfce5c5b5d776`. Linux `hetzner-dsm` in
`/root/wayland-p21`; Windows `SeanD@seandesktop` in `C:\ferrox-win-p21`, from a
single quiet run.

## Commits

| SHA | What |
|---|---|
| `39b11190` | Task 1 — the attribution corpus, its case table and its live drivers |
| `32cb2aef` | Task 1 — harness repair iteration 1, five defects the first instrumented run measured |
| `f2d186f6` | Task 1 — harness repair iteration 2, sibling failures made diagnosable |

## The corpus

Six entries, one per F21-03 lifecycle event, expressed as data in `cases.rs` and
executed in two modes by drivers that reach the real seams.

| Event | Siblings | Generations | In-process seam |
|---|---|---|---|
| reservation | 2 | 2 | `BudgetTracker::reserve` + `reserved_totals` |
| refund | 2 | 2 | `BudgetAuthorityCoordinator::bind` over a `SessionJournal`, dropped and rebound, then `release` |
| escalation | 2 | **3** | `BudgetTracker::extend_session` + `effective_session_limits` |
| approval | 2 | 2 | `ApprovalBridge::request_with_id` + `resolve_by_correlation` |
| cancellation | 2 | 2 | `DurableChildStore::transition(RequestCancel)` + `inspect` |
| result delivery | 2 | **3** | `DurableChildStore::transition(DeliveryStarted / DeliveryDelivered)` over `ChildDeliveryTarget` |

**At least two siblings, always** — asserted by
`every_case_runs_at_least_two_siblings`, not left to convention. One child under
one parent attributes correctly by accident because there is nowhere else for
anything to go. Escalation and delivery run three generations because their
question is which ANCESTOR an event rolls up to, not merely which peer owns it.
The delivery case goes further: two sibling grandchildren are bound for the
IDENTICAL `ParentChild` target, so delivering one and not the other is what
proves delivery is keyed by producer rather than by destination.

The crash-and-restart case is refund, and it is a real restart: the coordinator
is dropped (the crash), a second one is bound over the same journal file on disk
(the restart), and only then is one sibling's reservation released.

## Results

**Zero MISATTRIBUTED verdicts were measured — anywhere, in any mode, on either
platform.** Nothing caught the product putting a nested event on the wrong actor.

| Event | in-process (both platforms) | json-stream | rendered screen |
|---|---|---|---|
| reservation | **CORRECT** | NOT-OBSERVABLE | n/a |
| refund (crash + restart) | NOT-OBSERVABLE | NOT-OBSERVABLE | n/a |
| escalation | **CORRECT** | NOT-OBSERVABLE | n/a |
| approval | **CORRECT** | NOT-OBSERVABLE | Linux NOT-OBSERVABLE / Windows UNAVAILABLE |
| cancellation | **CORRECT** | NOT-OBSERVABLE | Linux NOT-OBSERVABLE / Windows UNAVAILABLE |
| result delivery | **CORRECT** | **observed correct live** | n/a |

Every aggregate `CASE` row reads `NOT-OBSERVABLE` because the aggregate is the
pessimistic fold; the per-mode table above is the substance.

**The live positive, stated precisely.** Every run in which the sibling pair
survived produced exactly two distinct `parent_call_id` values on the shipped
`--json-stream` wire — `spawn:0:anon` and `spawn:1:anon`, with `agent_name`
`corpus-sib-alpha` and `corpus-sib-beta` — and each sibling's own result sentinel
appeared under exactly one and never under the other. That is result-delivery
attribution proved on the real binary. `spawn_tool.rs:377` mints the per-task
key; `main.rs:4112` is the production call that turns `sub_agent_traces` on, so
it is not a test-only affordance.

**The in-process-pass-against-live-misattribution class has zero members.**
Where the product can be observed, the plumbing and the product agree. The
harness's `assert_mode_equivalence` fails on exactly that direction and never
fired.

## Findings

Three new HIGH, routed to Sean as open findings; three MEDIUM/LOW to BACKLOG.

**F21-04-01 (HIGH) — the host protocol carries no per-child observable for four
of the six events.** `ProtocolCommand` offers only a whole-turn `Stop`;
`BudgetExceeded` and `BudgetGrantResult` carry no actor; `ChannelSink` relays
only text, thinking, stream lifecycle, error and info, with `emit_tool_call` and
`emit_tool_result` deliberate no-ops. And `approval_required` arrives carrying
`call_id` and `correlation_id` but NO field naming the sibling that raised it —
so a host with two siblings running cannot tell whose gate it is answering. That
is the exact shape of this plan's own threat T-21-04-02. It is an observability
gap, not a demonstrated misattribution.

**F21-04-02 (HIGH) — a provider reservation handle does not survive a restart.**
After dropping and rebinding the coordinator over the same journal, both
siblings' `reserved_totals` read `(0, 0.0)` and `release` returned `false`. The
reservations did not lose their owner; they were not there.
`reconcile_restored_reservations_conservatively` and
`restored_reservation_reconciliation` exist to reconcile exactly this state, so
the intended behaviour is clearly the opposite of what was measured. Whether the
gap is in this corpus's binding of the durable path or in the product is NOT
settled — both permitted harness iterations were spent and a third is forbidden.
Escalated with its evidence rather than guessed at.

**F21-04-03 (HIGH) — two parallel `Spawn` siblings fail outright on a
journal-head collision, and the loser is left permanently faulted.** Found
because an attribution corpus is the first thing in this phase to run two
siblings at once against the shipped binary. Linux: 3 of 8 live runs. Windows:
**6 of 6** — at this SHA two parallel siblings did not once complete there. The
seam is `session_journal/reducer.rs:708`, a compare-and-swap on the journal head;
two concurrent siblings each capture it and the second loses. The loser does not
retry — `budget authority is permanently faulted` — both siblings die, and the
session is left with `turn … has nonterminal tool execution`. This is not an
attribution defect; it is a **parallel-delegation defect on the product's
advertised fan-out path**.

Checked twice before being called new, as the plan requires. Against
`21-03-REPAIR-SET.md`: no attribution red is downstream of F21-02-01, F21-02-02
or F21-02-03, because this corpus never drives tool authority, the policy gate or
the approval-posture resolver — nothing is double-counted. Against the Phase
20/20A known-red list: the cancellation case does not assert that descendants
died, so it never meets the deliberately-left Windows
`live_future_drop_reaps_descendant_job_tree`. The `prior cursor` string appears
nowhere in the handoff, BACKLOG, 21-02 or 21-03.

To BACKLOG: F21-04-04 (`ParentTurn` delivery unexercised, MEDIUM), F21-04-05
(`charge` does not block a session, LOW), F21-04-06 (`stream_end` is per stream
not per turn, LOW), and the windows-tui limitation (MEDIUM).

## The Windows TUI limitation

`LIMITATION :: windows-tui :: MEDIUM :: approval and cancellation as a human sees
them.` `pty_capture.rs` is `#![cfg(unix)]` because portable_pty's ConPTY backend
does not surface the child's stdout to the master end, and `support/pty.rs`
inherits the gate. Recorded `UNAVAILABLE` with its reason; no headless or
json-stream result was substituted for it. MEDIUM rather than HIGH because the
Linux rendered-screen leg reached the same `NOT-OBSERVABLE` verdict its
json-stream sibling did, so the Windows gap withholds a confirmation rather than
a distinct signal.

## The verdict

`CRITERION :: 1 :: NOT-MET` — the criterion says a child cannot widen ANY of
eleven restrictions. Tool authority is confirmed absent and DECLINED open;
approval's live closure is recorded NOT-CLOSED; `PolicyGate` is unreachable and
DECLINED open. Six of eleven dimensions hold in part by absence of a request
channel rather than by enforcement. Grading this MET-WITH-EXCEPTIONS would be the
narrowing this plan exists to avoid: a criterion that says *any* is not satisfied
by *most*.

**Cross-audited** (`21-04-t3-panel/PANEL.md`).
`PANEL-DECISION :: NOT-MET :: UNANIMOUS-ON-THE-EXTERNAL-LEGS`. `codex-sol`,
`gemini-3.1-pro` and `kimi-k3` each returned NOT-MET independently; the internal
adversarial pass argued for MET-WITH-STATED-EXCEPTIONS on three non-frivolous
grounds and lost on one that needs no panel: Criterion 1 is the only one of the
three carrying a universal over an ENUMERATED list, and its enumeration contains
a member the product's own unit test falsifies. Criteria 2 and 3 have gaps in
PROOF; Criterion 1 has a guard confirmed absent. The unanimity is discounted in
the panel record because all three members received the same framing.

`CRITERION :: 2 :: MET-WITH-STATED-EXCEPTIONS` — five of six events CORRECT in
process on both platforms, zero misattributions, delivery proved live on the
wire; excepted by F21-04-01, F21-04-02 and the Windows TUI gap.

`CRITERION :: 3 :: MET-WITH-STATED-EXCEPTIONS` — 21-02 asserted equivalence
structurally rather than by eye, and the two surfaces agreed on every decisive
dimension; excepted because six of eleven dimensions were NOT-EXPRESSIBLE or
NO-CHANNEL on at least one live combination, so the surfaces agree in substantial
part by both being unable to express the request.

`FANOUT :: DISTINCT-AND-COVERED` — restated verbatim from the census.

`SEAL :: NOT-CLAIMED`.

## Requirements

All four left OPEN, with a one-line justification each in `REQUIREMENTS.md` and
in `21-04-PHASE-VERDICT.md` §3. **None marked complete.** Three of the four are
open on live-evidence grounds rather than in-process failure — the distinction
this codebase learned when an entire permission crate passed its own tests while
no consumer called it.

## Gates

| Gate | Host | Result |
|---|---|---|
| `cargo fmt --all -- --check` | Mac | clean |
| `cargo clippy -p wcore-cli --all-targets -- -D warnings` | Hetzner | clean |
| binary build + execution check | Hetzner | `LIVE_BINARY_RUNS` |
| attribution corpus | Hetzner | 20/20 |
| full aggregate `--profile ci` | Hetzner | **11565/11565 passed**, 48 skipped, zero FAIL |
| clippy `-D warnings` FIRST | SEANDESKTOP | `CLIPPY_CLEAN` |
| binary build + execution check | SEANDESKTOP | `LIVE_BINARY_RUNS` |
| attribution corpus | SEANDESKTOP | 16/16, `NEXTEST_EXIT=0` |

Sixteen on Windows rather than twenty: the four `support::proving_ground::record`
unit tests are `#[cfg(unix)]`. No attribution case is missing. The aggregate rose
from 21-03's 11545 to 11565 — exactly the new binary's Linux test count, zero
regressions.

## Deviations

**1. [Rule 3 — blocking] The plan's pinned phase-base scope fence is
unsatisfiable as written.** Both Task 1 and Task 2 gate on
`git diff dd02a624 -- crates/` returning nothing outside the two corpora. That
base predates 21-03, which landed an AUTHORIZED production repair
(`execution_policy.rs` plus five contract-provenance files). Evaluated from the
pinned base the fence reports those six files and goes red before this plan runs
a line. Evaluated from 21-03's tip `18cef404` — this plan's true base — it
returns exactly the three attribution-corpus files and nothing else. Both results
are recorded rather than one being suppressed. The gate needed its base re-pinned
after 21-03 and was not; that is a plan defect, not a scope violation.

**2. [Rule 1 — harness bug, iteration 1 of 2] Five defects the first instrumented
run measured, each fixed with the finding recorded beside it in the code.**
`stream_end` fires per assistant stream, not per turn, so breaking on the first
one killed the process while the siblings were still running and recorded their
absence as a clean negative. `charge` records usage without blocking a session.
A durable budget authority refuses to bind without a canonical imported baseline.
A declared `turn_id` must name a turn the journal has seen. `ParentChild` names
the record's own parent child, not a handoff target.

**3. [Rule 1 — harness bug, iteration 2 of 2] An absent sibling read as silence
rather than as failure.** The Spawn tool's own failure output is now carried into
the evidence, which is how F21-04-03 became legible instead of looking like six
inconclusive rows.

**Both permitted harness iterations are now spent.** F21-04-02's cause is
therefore escalated rather than isolated — that is the termination criterion
working as designed, not a corner cut.

**4. Live-run bound.** Each live run is capped at 20 seconds. A run that exceeds
it is killed and records NOT-OBSERVABLE; nothing is ever recorded CORRECT because
it ran out of time. This is a bound on the harness, not a loosened gate.

## What phases 22 and 23 inherit

Stated in full in `21-04-PHASE-VERDICT.md` §5. The short form:

1. **Six HIGH findings enter Phase 22 open** — three declined by authorization in
   21-03, three new here.
2. **Provider, approval, and the `Some(..)` legs of depth, time, token and cost
   hold by ABSENCE OF A REQUEST CHANNEL, not by enforcement.** Phase 22's
   supervision and durable-goal work is exactly where such a channel is most
   likely to appear. 21-02's NO-CHANNEL canaries are built to go red on that day
   and are worth more than any currently-green assertion in the phase. Phase 22
   must not weaken or delete them.
3. **F21-04-03 blocks parallel supervision directly.** Two siblings already
   collide about half the time on Linux and every time on Windows, and the loser
   is permanently faulted rather than retried. Any Phase 22 fan-out proof meets
   this before it meets anything of its own.
4. **The host protocol cannot address or audit an individual child.** A
   supervision contract promising per-child pause, cancel or inspection over the
   wire needs a surface that does not exist yet.
5. **The Windows TUI stays undrivable** until ConPTY support lands.

## Recorded unknowns

Whether the TUI's rendered attribution stays stable enough to assert against as
the UI evolves — a real maintenance cost, accepted deliberately because
asserting only on the wire is what lets a product ship with correct plumbing and
a wrong screen. Whether an attribution property proved on Linux holds identically
on Windows for events whose only human-visible surface is the TUI — unsettleable
until ConPTY. Whether F21-04-02's restart gap is harness or product.

## Scope

Nothing was repaired. No production file under `crates/*/src` was touched. No
existing test was modified, renamed, re-gated, `#[ignore]`d, `#[allow]`ed or
deleted — the three new files contain zero of either. No production observability
hook was added to make a test possible. No new crate and no new dependency. No
seal claimed, no candidate prepared, no tag, no PR, no merge.

## Self-Check: PASSED

All eight created files exist on disk. All three commits are reachable
(`39b11190`, `32cb2aef`, `f2d186f6`). The scope fence from 21-03's tip returns
exactly the three attribution-corpus files. `git tag --points-at HEAD` is empty.
