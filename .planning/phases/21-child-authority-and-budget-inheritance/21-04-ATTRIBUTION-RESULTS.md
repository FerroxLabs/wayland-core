# 21-04 — Child Attribution Results

Phase 21, plan 21-04, Task 2. Success Criterion 2's measurement: *nested
reservation, refund, escalation, approval, cancellation and result delivery
remain attributable to the correct parent/session*.

**Corpus SHA, asserted on both hosts before any build step:**
`f2d186f6c3e77b99961171632fbbfce5c5b5d776`

**Hosts.** Linux `hetzner-dsm`, worktree `/root/wayland-p21`, detached at the SHA
above. Windows `SeanD@seandesktop`, worktree `C:\ferrox-win-p21`, detached at the
same SHA, driven from a single quiet run by a `-File` scheduled task with cargo
at `C:\Users\seand\.cargo\bin\cargo.exe` and clippy `-D warnings` FIRST.

**What this artifact is not.** It records no repair. Every finding below is
routed under the amended rules — CRITICAL and HIGH to Sean as open findings in
`21-04-PHASE-VERDICT.md`, MEDIUM and below to `.planning/BACKLOG.md`.

---

## 1. How to read a row

`CASE` is the aggregate for one lifecycle event on one platform. It is the
WORST of that event's modes, because a misattribution seen anywhere is a
misattribution and an event whose attribution no surface can show is not proved
by the surfaces that stayed silent. `MODE` rows carry the per-surface detail.

Five verdicts, closed set:

| Verdict | Meaning |
|---|---|
| `CORRECT` | The event landed on the actor that caused it, and on no other. |
| `MISATTRIBUTED` | It landed on the wrong actor. A red. |
| `NOT-OBSERVABLE` | The surface does not expose enough to tell the two apart. Recorded, never asserted weakly, and never repaired by adding a production observability hook. |
| `UNAVAILABLE` | Declared unavailable on this platform. Never a silent skip. |

**Every case ran with at least two siblings**, and escalation and result
delivery ran three generations deep. This is the corpus's load-bearing
construction rule, asserted by `every_case_runs_at_least_two_siblings` rather
than left to convention: one child under one parent attributes correctly by
accident, so a single-child corpus goes green on a system that ignores
attribution entirely.

---

## 2. Case outcomes

CASE :: attribution_reservation :: reservation :: linux :: NOT-OBSERVABLE
CASE :: attribution_refund :: refund :: linux :: NOT-OBSERVABLE
CASE :: attribution_escalation :: escalation :: linux :: NOT-OBSERVABLE
CASE :: attribution_approval :: approval :: linux :: NOT-OBSERVABLE
CASE :: attribution_cancellation :: cancellation :: linux :: NOT-OBSERVABLE
CASE :: attribution_delivery :: delivery :: linux :: NOT-OBSERVABLE
CASE :: attribution_reservation :: reservation :: windows :: NOT-OBSERVABLE
CASE :: attribution_refund :: refund :: windows :: NOT-OBSERVABLE
CASE :: attribution_escalation :: escalation :: windows :: NOT-OBSERVABLE
CASE :: attribution_approval :: approval :: windows :: NOT-OBSERVABLE
CASE :: attribution_cancellation :: cancellation :: windows :: NOT-OBSERVABLE
CASE :: attribution_delivery :: delivery :: windows :: NOT-OBSERVABLE

Every aggregate is `NOT-OBSERVABLE`, and the aggregate is the pessimistic fold.
That single column would be badly misleading read alone, so the per-mode table
below is the substance:

| Event | in-process (linux) | json-stream (linux) | rendered screen (linux) | windows |
|---|---|---|---|---|
| reservation | **CORRECT** | NOT-OBSERVABLE | not declared | see §5 |
| refund (crash + restart) | NOT-OBSERVABLE | NOT-OBSERVABLE | not declared | see §5 |
| escalation | **CORRECT** | NOT-OBSERVABLE | not declared | see §5 |
| approval | **CORRECT** | NOT-OBSERVABLE | NOT-OBSERVABLE | TUI UNAVAILABLE |
| cancellation | **CORRECT** | NOT-OBSERVABLE | NOT-OBSERVABLE | TUI UNAVAILABLE |
| result delivery | **CORRECT** | NOT-OBSERVABLE | not declared | see §5 |

**Five of six lifecycle events attribute CORRECTLY at the real in-process
seam.** Not one MISATTRIBUTED verdict was measured anywhere, on either platform,
in any mode. The sixth — refund across a crash and restart — is
`NOT-OBSERVABLE` for a specific and substantive reason recorded as F21-04-02.

The wire column is the finding. It is not that attribution is wrong there; it
is that for four of the six events the host protocol carries nothing that could
tell right from wrong.

---

## 3. Live evidence

Every live row carries its exact invocation, the mode the run PROVED it landed
in, and its captured transcript. A verdict from a run that never proved its mode
is withheld by the harness before it can be written down —
`crates/wcore-cli/src/main.rs` falls through from the TUI to the line REPL when
stdout is not a terminal, so a piped subprocess can otherwise report a verdict
for a surface it never exercised. json-stream mode is proved by the `ready`
frame nothing else in the product emits; the rendered screen by the full-screen
chrome only the TUI paints.

LIVE :: attribution_reservation :: linux :: json-stream :: wayland-core --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
LIVE :: attribution_refund :: linux :: json-stream :: wayland-core --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
LIVE :: attribution_escalation :: linux :: json-stream :: wayland-core --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
LIVE :: attribution_approval :: linux :: json-stream :: wayland-core --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
LIVE :: attribution_cancellation :: linux :: json-stream :: wayland-core --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
LIVE :: attribution_delivery :: linux :: json-stream :: wayland-core --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
LIVE :: attribution_approval :: linux :: tui :: wayland-core (bare, attached to a real PTY; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
LIVE :: attribution_cancellation :: linux :: tui :: wayland-core (bare, attached to a real PTY; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-linux.log
LIVE :: attribution_reservation :: windows :: json-stream :: wayland-core.exe --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-windows.log
LIVE :: attribution_refund :: windows :: json-stream :: wayland-core.exe --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-windows.log
LIVE :: attribution_escalation :: windows :: json-stream :: wayland-core.exe --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-windows.log
LIVE :: attribution_approval :: windows :: json-stream :: wayland-core.exe --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-windows.log
LIVE :: attribution_cancellation :: windows :: json-stream :: wayland-core.exe --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-windows.log
LIVE :: attribution_delivery :: windows :: json-stream :: wayland-core.exe --json-stream --provider anthropic (stdin: one `message` command; hermetic WAYLAND_HOME) :: .planning/phases/21-child-authority-and-budget-inheritance/evidence/21-04-t2-windows.log

The per-run raw transcripts (every wire frame, every rendered screen) are
written by the harness to `target/tmp/child-attribution-corpus/transcripts/` on
each host and named in each `MODE` row inside the captured platform logs above.

---

## 4. What the wire does and does not carry

Measured against the source, not assumed:

**It carries per-sibling identity, and that machinery WORKS.** Every run in
which the sibling pair survived produced exactly two distinct `parent_call_id`
values — `spawn:0:anon` and `spawn:1:anon` — with `agent_name`
`corpus-sib-alpha` and `corpus-sib-beta`, and each sibling's own result sentinel
appeared under exactly one of them and never under the other. That is result
delivery attributing correctly on the shipped wire, observed at this SHA in the
reservation, escalation and approval runs. `crates/wcore-agent/src/spawn_tool.rs:377`
mints the per-task key and `crates/wcore-cli/src/main.rs:4112` is the production
call that turns `sub_agent_traces` on for `--json-stream`, so the capability is
not a test-only affordance.

**It carries nothing per-child for four of the six events.**

| Event | Why the wire cannot answer |
|---|---|
| reservation | `ChannelSink` relays only text, thinking, stream lifecycle, error and info (`agents/channel_sink.rs:161-233`); `emit_tool_call` and `emit_tool_result` are deliberate no-ops. No budget counter crosses the relay. |
| refund | As above. `BudgetExceeded` is emitted at top level carrying `reason`, `observed` and `limit` and NO actor (`wcore-protocol/src/events.rs`). |
| escalation | `BudgetGrantResult` carries `request_id`, amounts and outcome — no child or session actor. |
| cancellation | `ProtocolCommand` has `Stop`, which is whole-turn, and no per-child variant at all (`wcore-protocol/src/commands.rs:142`). A host cannot address one sibling. |

**Approval is a distinct and sharper case.** The frames DO arrive: the
approval run produced one `approval_required` with `call_id` `toolu_mock` and
`correlation_id` `toolu_mock`, and the two siblings each took two provider
turns, so both were live and both were asked to mutate. The frame carries no
field naming which nested actor raised it. A host with two siblings running
therefore cannot tell whose gate it is answering, which is the precise shape of
threat T-21-04-02 in this plan's own register — an authority transfer wearing a
UI's clothes. It is recorded `NOT-OBSERVABLE` rather than `MISATTRIBUTED`
because nothing was observed landing on the wrong actor; the surface simply
cannot distinguish the two.

---

## 5. Windows

Driven from a single quiet run. Both registered Windows runners resolve to one
physical box and concurrent compile load has already corrupted one proof run in
this project, so nothing else was scheduled against `SEANDESKTOP` while this ran.
The run was launched as a `schtasks` `/TR` `-File` task rather than an SSH child,
because Windows OpenSSH kills session children when the connection closes and a
cold cargo build outlives any single ssh call.

| Gate | Result |
|---|---|
| SHA asserted before any build step | `f2d186f6c3e77b99961171632fbbfce5c5b5d776` |
| `cargo clippy -p wcore-cli --all-targets -- -D warnings`, FIRST | `CLIPPY_CLEAN` |
| `cargo build -p wcore-cli --bin wayland-core` | built |
| `.\target\debug\wayland-core.exe --help` | `LIVE_BINARY_RUNS` |
| `cargo nextest run -p wcore-cli --test child_attribution_corpus` | 16 run, **16 passed**, 0 skipped, `NEXTEST_EXIT=0` |

Sixteen rather than Linux's twenty: the four `support::proving_ground::record`
unit tests are `#[cfg(unix)]` and do not exist on Windows. No attribution case is
missing.

**In-process attribution is IDENTICAL to Linux.** Same five CORRECT
(reservation, escalation, approval, cancellation, result delivery), same one
NOT-OBSERVABLE (refund across a restart), same numbers in every detail string.
Attribution at the real seam is platform-independent on the evidence in hand.

**The rendered screen is DECLARED UNAVAILABLE**, and the harness recorded it as
`UNAVAILABLE` with its reason rather than skipping it or substituting the
json-stream result. See §7.

**The wire told us nothing on Windows, and the reason is a red.** All six
json-stream runs reported `2 of 2 sub-agent(s) failed or terminated early` with
the same journal-head collision as Linux — but on Windows it hit **6 of 6**
rather than roughly half. At the recorded SHA, two parallel `Spawn` siblings did
not once complete on this platform. That is F21-04-03 and it is a red reported,
not engineered around: the corpus recorded `NOT-OBSERVABLE` with the tool's own
failure text attached, rather than recording an absence as if it were a refusal.

Captured transcript: `evidence/21-04-t2-windows.log` (217 lines, carrying
`RUN_SHA`, `CLIPPY_CLEAN`, `LIVE_BINARY_RUNS`, `NEXTEST_EXIT=0`, and every `CASE`,
`TOPOLOGY`, `MODE` and `LIVE` row).

---

## 6. Mode equivalence

MODE-EQUIVALENCE :: CONSISTENT :: No in-process CORRECT stands against a live MISATTRIBUTED anywhere in the corpus, on either platform. Zero MISATTRIBUTED verdicts were measured at all. The divergence that exists is in OBSERVABILITY, not in enforcement: five events attribute correctly in process while four of them expose nothing on the wire that could confirm or refute it. That is a coverage gap, reported as F21-04-01, and calling it a mode-equivalence divergence would overstate it — the harness's `assert_mode_equivalence` fails only on the serious direction, in-process CORRECT against live MISATTRIBUTED, and it did not fire.

The in-process-pass-against-live-misattribution finding class this plan was
required to look for therefore has **zero members**. That is a real result and
worth stating plainly: where the product can be observed, the plumbing and the
product agree.

---

## 7. Platform limitation

LIMITATION :: windows-tui :: MEDIUM :: approval and cancellation as a human sees them. Both events are answered or watched by a person on a rendered screen, and `crates/wcore-eval-scenarios/src/pty_capture.rs` carries `#![cfg(unix)]` at line 63 because portable_pty's Windows ConPTY backend does not surface the spawned binary's stdout to the master end; `crates/wcore-cli/tests/support/pty.rs` inherits the gate. The rendered-screen mode is DECLARED `UNAVAILABLE` on Windows and is not reported as passing there, and no headless or json-stream result is substituted for it. Severity is MEDIUM rather than HIGH because the Linux rendered-screen leg reached the same `NOT-OBSERVABLE` verdict its json-stream sibling did, so the Windows gap withholds a confirmation rather than a distinct signal — but it is a real gap and it will stay open until the PTY driver supports ConPTY.

---

## 8. Findings

FINDING :: F21-04-01 :: HIGH :: The host protocol carries NO per-child observable for reservation, refund, escalation or cancellation, and no field on `approval_required` naming the sibling that raised it. `ProtocolCommand` offers only a whole-turn `Stop`; `BudgetExceeded` and `BudgetGrantResult` carry no actor; `ChannelSink` relays only text, thinking, stream lifecycle, error and info. Consequence for Success Criterion 2: attribution for five of its six named events is proved at the in-process seam ONLY, and a host application driving Core over the protocol cannot render, address or audit them per child. This is a product observability gap, not a demonstrated misattribution — nothing was seen landing on the wrong actor. Repair is out of scope here: this plan repairs nothing and adding an observability hook to make a test possible is explicitly forbidden.

FINDING :: F21-04-02 :: HIGH :: A provider reservation handle does not survive a process restart, so a refund cannot be attributed across a crash. Two siblings reserved through `BudgetAuthorityCoordinator` over a real `SessionJournal`; the coordinator was dropped (the crash) and rebound over the same journal file (the restart). After the rebind both siblings' `reserved_totals` read `(0, 0.0)` and `BudgetTracker::release` on sibling A's handle returned `false`. The reservations did not merely lose their owner — they were not there. `budget_authority.rs` exists precisely because runtime budget mutation is only useful if the same authority survives a crash, and `reconcile_restored_reservations_conservatively` and `restored_reservation_reconciliation` exist to reconcile exactly this state, so the intended behaviour is clearly the opposite of what was measured. Whether the gap is in the harness's binding of the durable path or in the product is NOT settled here — this plan's two permitted harness repair iterations are spent and its termination criterion forbids a third. It is escalated with its evidence rather than guessed at.

FINDING :: F21-04-03 :: HIGH :: Two parallel `Spawn` siblings fail outright on the shipped binary with a journal-head compare-and-swap collision, and the losing sibling's budget authority is left PERMANENTLY FAULTED. Measured on Linux in 3 of the 8 live runs in the recorded transcript, with 4 completing normally — a race. Measured on Windows in 6 of 6 json-stream runs: at this SHA two parallel siblings did not once complete on that platform. The `tool_result` reports `2 of 2 sub-agent(s) failed or terminated early` and each sibling's terminal carries `Sub-agent error: Session persistence authority unavailable: budget journal operation failed: invalid journal state transition: budget authority prior cursor does not match the current journal head`. The seam is `crates/wcore-agent/src/session_journal/reducer.rs:708`, which rejects a budget-authority append whose `prior_cursor.journal_sequence` no longer equals `state.last_seq` — two concurrent siblings each capture the head, and the second loses. The loser does not retry: its terminal reads `budget authority is permanently faulted: invalid journal state transition: budget authority prior cursor does not match the current journal head`. The session is then left carrying `turn ... has nonterminal tool execution`, i.e. the failure is not clean. NOT a known red: the string appears nowhere in the Phase 20/20A handoff, in BACKLOG, in 21-02-CORPUS-RESULTS or in 21-03-REPAIR-SET. NOT the same as 21-02's F21-02-08, which was a missing ephemeral vault under a hermetic home; the vault is configured here and the session starts cleanly. Sibling attribution of the FAILURE is correct (`msg_id` `spawn:1:anon:terminal`), so this is not itself a misattribution — it is the reason several attribution rows could not be measured live, and on its own merits it is a parallel-delegation defect on the product's advertised fan-out path.

FINDING :: F21-04-04 :: MEDIUM :: `ChildDeliveryTarget::ParentTurn` was not exercised in process. The journal reducer refuses a declaration carrying it unless `parent.turn_id` names a turn the journal has actually seen (`session_journal/reducer.rs:1597`), and this corpus declares children directly rather than through a live turn. `SessionOutbox` and `ParentChild { child_id }` were both exercised, the latter with two sibling grandchildren bound for the IDENTICAL target — the hardest form, since delivering one must not mark the other delivered. `ParentTurn` is recorded unexercised rather than faked with a synthetic turn, which would have proved attribution against a fixture instead of against the product. Routed to BACKLOG.

FINDING :: F21-04-05 :: LOW :: `BudgetTracker::charge` records usage and returns the cap error but does NOT add the session to the blocked set; only the admission paths do (`tracker.rs:910/921/932/951` for `reserve_turn`, `:1038-1083` for `settle_turn`). An escalation therefore cannot be requested after charging past a cap, only after being refused admission. Not a defect — `extend_session` gating on `NoExhaustedBudget` is deliberate — but the asymmetry is undocumented and cost this plan one harness iteration. Routed to BACKLOG.

FINDING :: F21-04-06 :: LOW :: `ProtocolEvent::StreamEnd` is emitted per assistant STREAM, not per turn, so a parent's first response already ends with one even though its tool call has not run. Any live driver that treats `stream_end` as "the turn is over" will kill the process while spawned children are still working and record their absence as if the topology never existed. This plan's first instrumented run did exactly that. Recorded so the next live harness does not rediscover it. Routed to BACKLOG.

### Reds checked against prior findings before being reported

Per this plan's rules every red was checked twice before being called new.

* Against `21-03-REPAIR-SET.md`: F21-02-01 (tool authority) and F21-02-03
  (PolicyGate reachability) are DECLINED and open, and F21-02-02's live closure
  is recorded NOT-CLOSED. None of them is upstream of any attribution row here —
  this corpus never drives tool authority, the policy gate, or the approval
  posture resolver. No attribution red is a consequence of an open Criterion-1
  finding, and none is double-counted as one.
* Against the Phase 20/20A known-red list: the cancellation case does NOT assert
  that descendants died, so it never meets the deliberately-left Windows red
  `live_future_drop_reaps_descendant_job_tree`. That red is not present in this
  corpus's results and is not reported here.

---

## 9. Gates

| Gate | Host | Result |
|---|---|---|
| `cargo clippy -p wcore-cli --all-targets -- -D warnings` | Hetzner | clean |
| `cargo build --locked -p wcore-cli --bin wayland-core` | Hetzner | built |
| `./target/debug/wayland-core --help` | Hetzner | executes |
| `cargo nextest run -p wcore-cli --test child_attribution_corpus` | Hetzner | 20/20 |
| `cargo nextest run --profile ci --no-fail-fast` (full aggregate) | Hetzner | 11565/11565 — see §9a |
| clippy `-D warnings` FIRST | SEANDESKTOP | clean |
| binary build + execution check | SEANDESKTOP | built, `LIVE_BINARY_RUNS` |
| attribution corpus | SEANDESKTOP | 16/16, `NEXTEST_EXIT=0` |

### 9a. Full aggregate

`cargo nextest run --profile ci --no-fail-fast` on Hetzner at the recorded SHA:

```
Summary [ 189.835s] 11565 tests run: 11565 passed (1 slow, 3 flaky), 48 skipped
```

Zero `FAIL` lines. **Zero regressions** — the attribution corpus adds tests and
breaks nothing. Against 21-03's recorded 11545 the count rises by 20, exactly the
attribution binary's test count on Linux.

The three flaky-then-passing tests are pre-existing and named rather than
annotated away: `wcore-cli::deterministic_openai_loop packaged_core_cancels_an_active_stream`
(FLAKY 3/3 — already `21-02-CORPUS-RESULTS.md` finding F21-02-10 and
`.planning/TEST-AUDIT.md:171`) and `wcore-agent::dangerous_lease_e2e_test dangerous_expiry_cancels_production_streaming_bash_process_tree`
(FLAKY 2/3). Neither is targeted by this plan and neither is reported as a Phase
21 finding. The observation 21-02 made still holds and is worth repeating: a live
corpus adds real binary spawns to the aggregate and plausibly tips
timing-sensitive cancellation tests rather than exposing new ones.

---

## 10. Scope

Nothing was repaired. No production file under `crates/*/src` was touched, no
existing test was modified, renamed, re-gated or deleted, and the count of
`#[ignore]`d tests is unchanged from the phase base. The only files this plan
adds under `crates/` are the three attribution-corpus test files.
