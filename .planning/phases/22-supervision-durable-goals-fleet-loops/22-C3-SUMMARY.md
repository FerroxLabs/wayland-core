# Phase 22 Criterion 3 — the adapter surface, and an honest grade

Lane `lane/22-c3`. Merge base `0b16f86791a707c614c14a1e1ee9f1a0c17d27d9` (captured once,
quoted everywhere). HEAD `bc047d56`. Proven on `hetzner-dsm`; live legs against
`wayland-core 0.12.25` release, Linux.

> **Criterion 3, verbatim:** *"Direct, ForgeFlows, Fleet, Council, and Anvil terminate
> through one canonical Goal transition with no nested verification/retry owner."*

**Grade: PARTIAL.** Not FAILED — there is now a construction, it is enforced by the
type system and the durable boundary rather than by convention, and it was driven
through the shipped binary. Not PASSED — **one of the five engines is reachable
through it from the product**, and an engine invoked outside a Goal still terminates
its own way. Details in "What is not done", below, which is the part that matters.

---

## 1. What the adapter surface is

`crates/wcore-agent/src/goal/strategy.rs`. Five adapters, one canonical transition,
and a chain closed at four independent links:

```
engine's own outcome type
  (ClimbOutcome | CouncilRunResult | WorkflowRunError | FleetOutcome | DirectOutcome)
      ↓  exactly one adapter, consuming LoopOwner<S> BY VALUE
StrategyTermination     ← no public constructor but the five adapters; not Deserialize; not Clone
      ↓  its only consumer
GoalKernel::finish_loop_owner   ← pub(crate)
      ↓
SessionEvent::GoalLoopOwnerFinished  ← SessionJournal::append refuses it; only the kernel mints one
```

It consumes `wcore_types::goal::GoalTerminalState` and does **not** extend it, per the
census's central finding that the taxonomy is a LIFT of Anvil's, not a sixth vocabulary.

Supporting durable work: `GoalLoopOwnerClaimed` / `GoalLoopOwnerFinished` events,
`GoalState.loop_owner` + `.loop_owner_epochs`, and — in `anvil/engine.rs` — the gate
evidence the verified path needs, which the engine had been measuring and throwing away.

## 2. Which engines terminate canonically, and which do not

| Engine | Adapter | Terminates canonically under a Goal | Reachable that way from the product |
|---|---|---|---|
| Fleet | `from_fleet` (bound at `ShardSummary`, never the caller-chosen `T`) | **yes**, proven live | **yes** — `wayland-core goal run --terminate` |
| Direct | `from_direct` | yes, proven in tests | **no** |
| ForgeFlows | `from_forgeflows` | yes, proven in tests | **no** |
| Council | `from_council` | yes, proven in tests | **no** |
| Anvil | `from_anvil` (the only route to `Verified`) | yes, proven in tests | **no** |

`each_of_the_five_strategies_produces_exactly_one_canonical_transition` drives all five
to a *different* terminal category each and counts `GoalLoopOwnerFinished` records off
the raw chain — 1 per engine, so the count is not an artifact of one shared happy path.

## 3. Enforced, or conventional? Clause by clause

Enforcement is over the **Goal lifecycle**, not over engine invocation. Stated exactly:

> Once a Goal has claimed a loop owner — which is every Goal a strategy runs — it
> cannot reach a terminal state except through one canonical transition produced by
> that owner, and the owner is read from the durable record, not a call stack.

| Clause | Mechanism | Kind |
|---|---|---|
| exactly one transition, not zero | the engine closure's return type IS `StrategyTermination`; `run_*` always terminates with it | **compile** |
| not two | reducer refuses a second terminal; live claim + epoch fence | **durable** |
| no other route to a terminal | reducer refuses a plain `GoalTerminated` while a claim exists; `finish_loop_owner` is `pub(crate)`; `append` refuses every `Goal*` variant | **durable + compile** |
| wrong-strategy adapter | `from_anvil(LoopOwner<CouncilTag>)` is a type error | **compile**, `compile_fail` doctest |
| no retry wrapper around Anvil | `LoopOwner` is non-`Clone` and moved into the adapter; a loop is use-after-move | **compile**, `compile_fail` doctest |
| verified needs real gate evidence | `from_anvil` consumes `ClimbOutcome::gate_observation`, which only the engine can produce; no adapter parameter can supply one; reducer refuses `Verified` for a strategy with no host-observed owner | **compile + durable** |
| nested loop owner refused | live claim refuses a second; Goal left non-terminal and resumable | **durable** |
| strategy from the durable record | `run_*` reads `goal.authority.strategy`; reducer refuses a mismatching claim | **durable** |
| **an engine invoked outside any Goal** | **nothing** | **convention** |

That last row is the honest one and is restated in §6.

**How I proved the compile-fail gate is not self-passing.** Removed the retry loop from
the `from_anvil` doctest so the snippet compiles; the gate went **RED**
(`rc=101`, `test result: FAILED. 1 passed; 1 failed`), then restored, `git diff` clean.
Two further permanent falsification tests: the tag-collision detector run against a
deliberately-collided list, and `canonical_transitions` shown returning **0** for a Goal
that never ran a strategy — without which every "exactly one" assertion is a tautology.

## 4. Gate results — real numbers, read back

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | rc=0, 0 bytes |
| `cargo check --workspace --all-targets` | rc=0, **ERRCOUNT=0** (run because new `SessionEvent` variants can break downstream exhaustive matches) |
| `cargo nextest run -p wcore-agent` | **3024 run, 3024 passed**, 11 skipped, rc=0 |
| `cargo nextest run -p wcore-agent --test goal_strategy_test` | **17 run, 17 passed**, 0 skipped |
| `cargo nextest run -p wcore-agent --lib -E 'test(/goal::strategy::tests/)'` | **5 run, 5 passed** |
| `cargo nextest run -p wcore-types` | **142 run, 142 passed** |
| `cargo test --doc -p wcore-agent -- goal::strategy` | **2 passed**, 0 failed (nextest does NOT run doctests) |
| `cargo clippy -p wcore-agent -p wcore-cli --all-targets --all-features -- -D warnings` | rc=0, **ERRCOUNT=0** |
| `cargo nextest run -p wcore-cli` | 2307 run, **2 failed** — see below |

**The two `wcore-cli` failures are pre-existing and were measured at BASE, not assumed.**
`release_binary_smoke::release_binary_ready_event_advertises_plugin_capabilities` and
`plugin_discovery_e2e::ready_event_has_plugin_capability_flags`, both asserting
`capabilities.browser_suite`. At `0b16f867`: `4 tests run: 2 passed, 2 failed` — the
same two. Nothing in this lane touches plugin discovery.

Clippy's 4 pre-existing errors in `journey.rs` were neither inherited nor fixed:
that file is in `wcore-eval-scenarios`, a different crate from anything I touched.

## 5. Live transcript — the shipped binary, not a harness

`wayland-core 0.12.25` release, `/root/wayland-22c3/target/release/wayland-core`.

**The live run found a defect 3021 green tests could not.** The first invocation died at
`session journal writer lease is already held`: `goal run --terminate` opened one
`SessionJournal` handle for `GoalFleetDriver` and a second for `GoalLoop`, and an
independent second open fails closed by design. Every real invocation would have died
after recovery and before the first wave. No test caught it because every test builds a
single driver, so nothing in-process had ever opened the journal twice. Fixed at
`60c919b0` by cloning the one handle.

Canonical transition, driven end to end:

```
GOAL: run pid=3683975 goal=g-live
GOAL: recovery=resumed iterations=0 resume_count=1 resumable=true revoked=0 drained=0
GOAL-EXEC: task=t3 key=idem-t3 produced=yes
GOAL-EXEC: task=t2 key=idem-t2 produced=yes
GOAL-EXEC: task=t1 key=idem-t1 produced=yes
GOAL: wave=0 shards=2 claimed=3 completed=3 failed=0 indeterminate=0 abandoned=0 delivered=3
GOAL: run_complete waves=1 iterations=1 completed=3 delivered=3
GOAL: canonical_transition strategy=fleet
      terminal=Terminated { terminal: PartiallyCompleted { completed: 3, failed: 0 } }
      cursor_seq=Some(22)
```

**The claim is durable, proven by killing the thing that held it.** `kill -9` on the
process group mid-wave: **9 descendants → 0**. A fresh process replayed the chain:

```
lifecycle: {"state": "running"}
loop_owner: {"strategy": "fleet", "epoch": 1, "lease_expires_unix_ms": ...}
```

**Kill → refuse → supersede → terminate, all through the product:**

```
A. immediately (lease live):  "loop owner Fleet (epoch 1) is already live;
                               a nested loop owner is refused"        EARLY_RC=1
B. past the lease:            wave=0 claimed=4 completed=4 failed=0
                              canonical_transition strategy=fleet
                              terminal=PartiallyCompleted{completed:4,failed:0}
                              cursor_seq=Some(48)                      LATE_RC=0
   final: lifecycle terminated, loop_owner absent, loop_owner_epochs=2
   GOAL-EFFECTS: total=4 distinct=4   (gate --expect 4 → rc=0; --expect 99 → rc=1)
```

**22-01's M1 byte-identity survives, proven live rather than argued.** A Goal that never
claimed an owner serialises with **neither** new field: `loop_owner present: False`,
`loop_owner_epochs present: False`.

## 6. What is not done — read this part

1. **Four of the five engines have no product path through the canonical transition.**
   Direct, ForgeFlows, Council and Anvil have adapters and passing tests; only Fleet is
   wired into a shipped verb. On the strict reading of "the five engines terminate
   through one canonical Goal transition", four of them do so only in tests.
2. **An engine invoked outside any Goal is unenforced.** `run_climb`, `drive_council`,
   `WorkflowRunner::run`, `FleetDispatcher` and `Engine::run` still return their own
   types and can be called with no Goal in sight. Routing them through a Goal is
   **convention**. The measured reason is in `22-C3-NOTES.md` §M1: threading a loop-owner
   token through those entry points requires editing **≥40 existing test call sites**,
   and 22-02 Task 3's own `<done>` forbids modifying existing tests. Closing this needs
   a decision that overrides that constraint — it is a Sean-level scope call, not
   something to slip in.
3. **`--terminate` is opt-in.** It has to be: `goal run` is the verb 22-03's kill/restart
   proof re-enters, and terminating on every run would break that proof.
4. **Linux only.** No Windows leg was taken for any of this.
5. **Two taxonomy gaps found and reported rather than papered over.**
   (a) There is no "completed but unchecked" category. Direct has no verification owner
   at all, so a completed Direct run maps to `NeedsEscalation` — under-claiming, since
   `SelfChecked` would assert checks that never ran. (b)
   `PartiallyCompleted{completed:N, failed:0}` is the lossless honest encoding of a clean
   Fleet/ForgeFlows/Council run, but the variant *name* reads as a partial failure. The
   payload is exact; the name is not. Both want a later phase, not a stretched meaning now.
6. **22-02 Task 2's decision record was never written and I did not write one.** I chose
   the adapters-only option and recorded the reasoning in `22-C3-NOTES.md` §M1/§M4 instead
   of the `22-02-EVIDENCE/decision/` directory the plan specifies.

## 7. HIGH found and fixed — in my own construction, by the live kill

**The loop-owner claim originally had no lease, so a dead owner deadlocked its Goal
permanently.** After `kill -9` the epoch-1 claim was live with nobody holding it and
every restart was refused forever. Task claims in the same 22-03 ledger already carried a
lease for exactly this; the loop-owner claim did not. That was an asymmetry, not a design.

Fixed without weakening the nesting refusal: a **live** claim still refuses a second
owner; only an **expired** one is superseded. Reclaim is safe because of the epoch, not
in spite of it — `GoalLoopOwnerFinished` requires the live epoch, so a resurrected
predecessor cannot terminate a Goal it no longer holds
(`a_superseded_owner_cannot_terminate_the_goal_it_no_longer_holds`). A second live run
then caught that `--lease` was reaching task claims but not the loop-owner claim, which
would have stranded a killed Goal for a hidden 60s default (`bc047d56`).

## 8. For the orchestrator to serialize

- **Shared fence untouched.** `git diff $BASE -- crates/wcore-cli/src/lib.rs
  crates/wcore-cli/src/main.rs` is **empty**.
- **`crates/wcore-cli/src/goal_cmd.rs` is shared with lane `22-wire`.** My edits are
  additive: one flag, one options field, one `if options.terminate` block, one handle
  clone. Merge this lane after `22-wire` if both are outstanding.
- **New `SessionEvent` variants.** `GoalLoopOwnerClaimed` / `GoalLoopOwnerFinished` enter
  additively; `cargo check --workspace --all-targets` is clean. No contract fixture was
  regenerated and `wcore-contract generate` was **not** run.
- **`ClimbOutcome` gains `gate_observation`.** All its struct literals are inside
  `anvil/engine.rs`; no test constructs one.

## 9. Files

Changed vs `$BASE`: `goal/strategy.rs` (new), `goal/kernel.rs`, `goal/mod.rs`,
`orchestration/anvil/engine.rs`, `session_journal.rs`, `session_journal/model.rs`,
`session_journal/reducer.rs`, `tests/goal_strategy_test.rs` (new),
`wcore-cli/src/goal_cmd.rs`. No existing test was modified, renamed, re-gated or deleted.
