# 22-C3 working notes — Criterion 3, lane `lane/22-c3`

Append-only. Committed after every measurement so a watchdog kill costs minutes, not hours.

Merge base (captured once, quoted everywhere):
`BASE=0b16f86791a707c614c14a1e1ee9f1a0c17d27d9`

Mac worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-22-c3`
Hetzner worktree: `/root/wayland-22c3` (detached at `0b16f867`, created 2026-07-28)
Hetzner: 96 cores, load 0.68, 745G free at start.

---

## M0 — what already exists (read, not assumed)

| Artifact | State |
|---|---|
| `wcore_types::goal::GoalTerminalState` | LANDED. 14 variants, `#[non_exhaustive]`, lifted from Anvil's `TerminalState` + `Unpriced` / `Exhausted{ExhaustionKind}` / `PartiallyCompleted`. |
| `wcore_types::goal::GoalStrategy` | LANDED. 5 variants + `ALL: [_;5]` + `can_produce_host_observed_evidence()` (true only for Anvil). |
| `wcore_types::goal::VerifiedTerminal` / `HostGateObservation` | LANDED. Neither is `Deserialize`; `from_host_observed_gate` is the only route to `Verified` and returns `None` for every non-Anvil strategy. |
| `GoalKernel` (`crates/wcore-agent/src/goal/kernel.rs`) | LANDED. Sole writer of `SessionEvent::Goal*`; `SessionJournal::append` refuses every Goal variant (`session_journal.rs:389-403`). `terminate()` refuses `Verified`; `terminate_verified()` requires a `VerifiedTerminal`. |
| Reducer durable half | LANDED. `reducer.rs:322` refuses a `verified` terminal whose goal's recorded strategy has no host-observed verification owner. |
| **The adapter surface** | **DOES NOT EXIST.** No `goal/strategy.rs`. This is the whole of the remaining Criterion-3 work. |
| 22-02 Task 2 decision record | **DOES NOT EXIST.** `22-02-EVIDENCE/decision/` was never created. The option authorising Task 3 was never chosen. I must choose it (Decide-don't-park) before implementing. |

Existing callers of `GoalKernel::terminate*`: **tests only** (`goal_kernel_test.rs`,
`goal_fleet_ledger_test.rs:599`, `multi_day_journey_test.rs:442`). No production
caller. Measured with `grep -rn "\.terminate(" crates --include='*.rs'`. This means
narrowing the kernel's terminate signature breaks no production code.

---

## M1 — THE DETERMINATION: is full enforcement buildable from the real call sites?

This is the measurement the coordinator asked to be written down before any build,
because it is the valuable output whether or not the build follows.

"Full enforcement" = an engine cannot be *invoked at all* except under a Goal loop
owner, i.e. every one of the five entry points takes a token only `GoalLoop` can mint.

### Measured entry points and their call sites

| Engine | Entry point | Production call sites | Test call sites |
|---|---|---|---|
| Anvil | `run_climb` (`anvil/engine.rs`) | 1 — `anvil/forge.rs:821` | 3 — `tests/anvil_forge_transaction.rs:186,239,382` |
| Anvil | `drive_climb_full` (`anvil/forge.rs:630`) | 2 — `anvil/tool.rs:131`, `wcore-cli/src/anvil.rs` | 1 — `tests/anvil_forge_transaction.rs:697` |
| Council | `drive_council` (`council/driver.rs`) | 2 — `engine.rs:19825`, `wcore-cli/src/crucible.rs` | (via `run_council`) |
| Council | `run_council` (`council/run.rs`) | reached via `drive_council` | 10+ — `tests/crucible_council.rs:187,241,291,319,354,379,405,443,492,651` |
| ForgeFlows | `WorkflowRunner::new` / `::run` | workflow module + `slash`/`orchestration` wiring | 12+ across `pipeline_test.rs`, `forgeflows_live_relay_test.rs`, `workflow_e2e.rs`, `workflow_runner_test.rs`, `workflow_limits_test.rs` |
| Fleet | `FleetDispatcher::new` (`wcore-swarm/src/fleet.rs`) | `goal/fleet.rs` (`GoalFleetDriver`, already Goal-bound) | 12+ in `goal_fleet_wire_test.rs`, `fleet_dispatcher_wired_test.rs` |
| Direct | `Engine::run` (`engine.rs`) | the entire CLI/TUI/protocol surface | very large |

### Determination

**Full enforcement by token-threading the five entry points is NOT buildable in this
lane, and the reason is not effort — it is a hard constraint the plan itself sets.**

1. Adding a `LoopOwnerToken` parameter to `run_climb`, `drive_climb_full`,
   `run_council`, `drive_council`, `WorkflowRunner::run` and `FleetDispatcher`
   requires editing **≥40 existing test call sites** so they construct a token.
   22-02 Task 3's `<done>` says verbatim: *"No existing test was modified, renamed,
   re-gated or deleted."* Threading the token modifies ~40 of them. The plan
   forbids the only mechanism that would deliver full enforcement.
2. `Engine::run` (Direct) is reached from the CLI, the TUI, the JSON-stream
   protocol, every sub-agent spawn, and every other engine (the census records
   Direct as "the leaf — every other engine spawns Direct runs beneath itself").
   Requiring a Goal token there makes a Goal mandatory for every turn the product
   takes, which is a product-level change far outside a criterion-closing lane.
3. `AGENTS.md §3`: *"every changed line traces directly to the user's request"* and
   *"do not refactor code that works"*; 22-02 Task 3 repeats it — *"the workflow
   runner, the council driver and the Anvil engine are all mature and all
   load-bearing"*.

**So the honest ceiling for this lane is: enforcement of the canonical transition
over the GOAL LIFECYCLE, not over engine invocation.** Stated precisely, and this is
the claim I will try to make structural and will grade against:

> Once a durable Goal exists, it cannot reach a terminal state except through
> exactly one canonical transition produced by exactly one loop owner, and the
> loop owner is read from the durable record rather than a call stack.

What that does NOT give, and what I will say plainly in the SUMMARY:

> An engine invoked outside any Goal still returns its own type and terminates its
> own way. Routing an engine invocation through a Goal is, at the un-migrated call
> sites above, **convention**.

Criterion 3's words are *"terminate through one canonical **Goal** transition"* —
which is about Goal termination, not engine invocation. I will report both readings
and grade against the stricter one.

---

## M2 — the construction I am building (design, before code)

`crates/wcore-agent/src/goal/strategy.rs`:

* `StrategyTag` — sealed trait, 5 zero-sized tags (`Direct`, `ForgeFlows`, `Fleet`,
  `Council`, `Anvil`), each carrying `const STRATEGY: GoalStrategy`.
* `LoopOwner<S: StrategyTag>` — non-`Clone`, non-`Copy`, no public constructor.
  Minted ONLY by `GoalLoop::run_*` after reading the strategy off the durable Goal
  record. Consumed **by value** by the matching adapter.
* `StrategyTermination` — opaque (private fields), non-`Clone`, not `Deserialize`,
  `#[must_use]`. Its ONLY constructors are the five adapters. Its ONLY consumer is
  `GoalKernel::terminate_strategy`.
* Five adapters, each taking `LoopOwner<ThatTag>` by value:
  `from_direct`, `from_forgeflows`, `from_fleet`, `from_council`, `from_anvil`.
* `GoalLoop::run_<strategy>(goal_id, |owner| async { ... })` — claims the loop
  owner durably, invokes the closure once, terminates with what comes back.

Why each criterion clause becomes structural rather than conventional:

| Clause | Mechanism | Kind |
|---|---|---|
| exactly one transition, not zero | the closure's return type IS `StrategyTermination`; `run_*` always terminates with it. An early return that skips termination cannot typecheck. | compile |
| not two | reducer `require_goal_live` refuses a second `GoalTerminated`. | runtime, pre-existing |
| no other way to terminate | `GoalKernel::terminate`/`terminate_verified` narrowed to `pub(crate)`/consume-only-`StrategyTermination`; `SessionJournal::append` already refuses `GoalTerminated`. | compile |
| wrong-strategy adapter | `from_council(LoopOwner<Anvil>)` is a type error. | compile |
| no retry wrapper around Anvil | `LoopOwner` is non-`Clone` and moved into the adapter; a `for` loop calling the engine twice is use-after-move. | compile |
| verified needs real gate evidence | `from_anvil` derives the digest from the pinned `GateClosure` the parent executed; the other four adapters route through `from_host_observed_gate`, which returns `None` for their strategy; reducer refuses it durably too. | compile + runtime |
| nested loop owner refused | durable `GoalLoopOwnerClaimed` / `Released` record + reducer refusal; leaves the Goal non-terminal and resumable. | runtime, durable |
| strategy read from the record | `run_*` reads `goal.authority.strategy` and refuses a mismatch. | runtime |

Compile-fail proofs use **rustdoc `compile_fail` doctests** — no new dependency
(`trybuild` is absent from `Cargo.lock`, confirmed). NOTE: `cargo nextest` does NOT
run doctests; they need `cargo test --doc -p wcore-agent` and the count read back.

---

## M3 — traps carried into every gate in this lane

* Read `N passed` back. `cargo test -p X <filter>` matching nothing exits 0 having
  run zero tests. Run by `--test <file>`, never by filter.
* `cargo test --doc` is the only thing that runs `compile_fail` doctests.
* Clippy is RED at base with 4 pre-existing errors in `journey.rs` (another lane's
  file). Do not fix, silence or inherit; scope clippy to the crates I touch.
* `desktop_contract_corpus` is expected red — structural. Do NOT run
  `wcore-contract generate`.
* Prove every gate can fail before trusting it.

---

## M4 — design revision after measuring the back door (committed before coding)

Measured: `GoalKernel::terminate(goal_id, GoalTerminalState)` is called by three
EXISTING test files (`goal_kernel_test.rs:237,325,369,378,479`,
`goal_fleet_ledger_test.rs:599`, `multi_day_journey_test.rs:442`). Narrowing its
signature would modify existing tests, which 22-02 Task 3 `<done>` forbids.

So the back door is closed **durably in the reducer instead of by signature**:

* New durable events `GoalLoopOwnerClaimed { goal_id, strategy, epoch }` and
  `GoalLoopOwnerFinished { goal_id, epoch, terminal }`.
* `GoalLoopOwnerFinished` releases the claim AND terminates in ONE event, so there
  is no window between release and terminate for a racing plain terminate.
* The reducer REFUSES a plain `GoalTerminated` while a loop-owner claim is live.
* The reducer REFUSES a second `GoalLoopOwnerClaimed` while one is live — the
  nesting refusal — and leaves the Goal non-terminal and resumable.

Resulting property, stated exactly:

> For a Goal that has claimed a loop owner — which is every Goal a strategy runs —
> the canonical strategy transition is the ONLY route to a terminal state, refused
> durably by the reducer. A Goal that never claims an owner can still be terminated
> by hand, but by definition no engine ran it.

`GoalState.loop_owner: Option<GoalLoopOwner>` carries
`#[serde(default, skip_serializing_if = "Option::is_none")]` so 22-01's M1
byte-identity property survives: a Goal with no claim serialises exactly as before.

Measured blast radius of the Anvil subordination change: **every `ClimbOutcome { .. }`
struct literal is inside `engine.rs`** (lines 584, 665, 688, 705 + the unit-test
helper at 870). Zero test files construct one. So adding a field is contained.
`grep -rn "ClimbOutcome {" crates --include='*.rs'` → 11 hits, all in engine.rs.

### Anvil: the evidence gap I found, and the minimal subordination that closes it

`stability_holds()` (engine.rs:603) computes the observed pass count and then
**throws it away**, returning `bool`. `ClimbOutcome` therefore carries no stability
evidence, and no honest `HostGateObservation` can be built from it — an adapter
supplying `stability_repeats` would be paraphrasing, which is exactly what Test 6
forbids.

Minimal fix, inside `engine.rs` only: `stability_holds` returns the observed pass
count; `check_keepable` builds a real `HostGateObservation` from the gate report's
own `score()`/`total()`, the observed passes, and `params.gate_closure_digest`; and
`ClimbOutcome` gains `gate_observation: Option<HostGateObservation>`, set ONLY on the
keepable path and `None` at every other exit. The adapter then never constructs an
observation — it forwards one the engine measured.

### Terminal mapping, decided from the census (recorded so it can be argued with)

| Engine outcome | Canonical terminal | Why |
|---|---|---|
| Direct completed | `NeedsEscalation` | Direct has NO verification owner. `SelfChecked` would claim self-generated checks ran; none did. Under-claim, never over-claim. |
| Direct turn limit / context too long | `Exhausted{Resource}` | a resource envelope ran out |
| Direct `UserAborted` / cancel | `Cancelled` | |
| ForgeFlows `Ok` | `PartiallyCompleted{completed,failed}` from `StageResult::is_error` | lossless; makes no evidence claim |
| ForgeFlows `SchemaValidationFailed` | `Exhausted{Quality, attempts}` | the census's headline distinction |
| ForgeFlows `DispatchBudgetExceeded` | `Exhausted{Resource, attempted}` | the other half of it |
| ForgeFlows `StageFailed{partial}` | `PartiallyCompleted` from the partial | the partial payload is not discarded |
| ForgeFlows graph faults | `Blocked{reason}` | never started |
| Fleet `Ok(shards)` | `PartiallyCompleted{Σsuccesses, Σfailures}` | bound at `ShardSummary`, NOT at the caller-chosen `T` (census §3 finding) |
| Fleet `Timeout` | `TimedOut` | |
| Fleet `Shard`/`Topology` | `Blocked{reason}` | |
| Council `UnpriceableRoster` | `Unpriced{detail}` | the census's single most-cited need |
| Council `OverBudget`/`DailyBudgetExhausted` | `Exhausted{Resource}` | |
| Council `InsufficientProposals` | `Exhausted{Quality, got}` | |
| Council `Council{outcome}` | `PartiallyCompleted{chosen_from, skipped}` | `skipped` survives — census §4 |
| Council `Direct{..}` | `NeedsEscalation` | one model answer, no verification owner |
| Anvil `TerminalState::Verified` + real observation clearing the bar | `Verified` | the ONLY route |
| Anvil `Verified` whose observation does NOT clear the bar | `NeedsEscalation` | refusal, not a downgrade-but-still-verified |
| Anvil other 9 states | 1:1 | the taxonomy was lifted from them |
| Anvil `EngineError` | `Blocked{reason}` | aborted before terminal |

**Two taxonomy gaps found and NOT papered over** (reported, not fixed — the task
says the taxonomy is consumed, not extended):
1. There is no "completed but unchecked" category. Direct is exactly that, and
   `NeedsEscalation` is the least-wrong home for it.
2. `PartiallyCompleted{completed:N, failed:0}` is the honest lossless encoding of a
   clean Fleet/ForgeFlows/Council run, but the variant NAME reads as a partial
   failure. The payload is exact; the name is not. Worth a rename or a
   `Completed{unchecked}` variant in a later phase.

---

## M5 — built, and what the gates actually showed

Commits so far: `0e33c08d` notes → `4ff65de4` anvil evidence → `26be00cd` adapter surface
→ `09d5acf8` import fix → `74a6a6c0` test suite → `f7f72dc7` warning → `9db73178` CLI wiring
→ `60c919b0` writer-lease fix.

| Gate | Command | Result (read back, not assumed) |
|---|---|---|
| fmt | `cargo fmt --all -- --check` (Mac) | rc=0, 0 bytes of diff |
| compile | `cargo check -p wcore-agent --all-targets` | rc=0, 0 errors |
| strategy suite | `cargo nextest run -p wcore-agent --test goal_strategy_test` | **17 tests run: 17 passed, 0 skipped** |
| whole crate | `cargo nextest run -p wcore-agent --no-fail-fast` | **3021 run: 3021 passed, 11 skipped**, rc=0 — no regression |
| clippy | `cargo clippy -p wcore-agent --all-targets --all-features -- -D warnings` | rc=0, **ERRCOUNT=0**. `journey.rs` is in `wcore-eval-scenarios`, a different crate — the 4 pre-existing errors were neither inherited nor touched |
| compile-fail proofs | `cargo test --doc -p wcore-agent -- goal::strategy` | **2 passed**, 0 failed |

### The compile-fail gate was FALSIFIED, not merely run

Removing the retry loop from the `from_anvil` doctest (so the token is used once and
the snippet compiles) made the gate go **RED**: `FAILED_RC=101`,
`test result: FAILED. 1 passed; 1 failed`. Restored in the same run; `git diff --stat`
clean afterwards. So the doctest genuinely detects the move-once property rather than
passing because rustdoc ignored it.

Two runtime falsification tests are permanent, not one-off: the tag-collision detector
is run against a deliberately-collided list, and `canonical_transitions` is shown
returning **0** for a Goal that never ran a strategy — without which every "exactly
one" assertion would be a tautology.

## M6 — LIVE, against `wayland-core 0.12.25` release on hetzner

### The live run found a defect the whole green suite could not

First live invocation died at `session journal writer lease is already held`.
`SessionJournal::open` takes an exclusive cross-process lease and a second open fails
closed by design; `goal run --terminate` opened one handle for `GoalFleetDriver` and a
second for `GoalLoop`. **Every real invocation would have died after recovery and
before the first wave**, while 3021 tests stayed green — because every test builds a
single driver, so nothing in-process ever opened the journal twice. Fixed in
`60c919b0` by cloning the one handle.

### Transcript — the shipped binary driving the canonical transition

```
GOAL: run pid=3683975 goal=g-live
GOAL: recovery=resumed iterations=0 resume_count=1 resumable=true revoked=0 drained=0
GOAL-EXEC: task=t3 key=idem-t3 produced=yes
GOAL-EXEC: task=t2 key=idem-t2 produced=yes
GOAL-EXEC: task=t1 key=idem-t1 produced=yes
GOAL: wave=0 shards=2 claimed=3 lost=0 completed=3 failed=0 indeterminate=0 abandoned=0 delivered=3
GOAL: run_complete waves=1 iterations=1 completed=3 delivered=3
GOAL: canonical_transition strategy=fleet
      terminal=Terminated { terminal: PartiallyCompleted { completed: 3, failed: 0 } }
      cursor_seq=Some(22)
```
`goal status` then replays `{"state":"terminated","terminal":{"state":"partially_completed","completed":3,"failed":0}}`,
and a second `goal run` reports `recovery=already-terminal ... resumable=false`.

### Byte-identity (22-01 M1) survives — proven live, not argued

A Goal that never claimed an owner serialises with **neither** new field:
`loop_owner present: False`, `loop_owner_epochs present: False`.

### The claim is DURABLE, proven by killing the thing that held it

`kill -9` on the process group mid-wave: **9 descendants → 0**. A FRESH process then
replays the chain and finds the claim still there:

```
lifecycle: {"state": "running"}
loop_owner: {"strategy": "fleet", "epoch": 1}
loop_owner_epochs: 1
```

That is "recorded on the Goal, not held in a call stack" demonstrated against the one
event a call stack cannot survive. The restart's second claim was refused —
`loop owner Fleet (epoch 1) is already live; a nested loop owner is refused`, exit 1 —
and the Goal stayed `resumable=true` and non-terminal.

## M7 — HIGH, found in my own construction by the live kill

**The loop-owner claim has no lease, so an owner that dies deadlocks its Goal forever.**

The refusal above is correct in-process and wrong across a crash: after `kill -9` the
epoch-1 claim is live with nobody holding it, and no restart can ever claim or
terminate. Task claims in the 22-03 ledger already solve exactly this with a lease
(`--lease 60s`, "a claim whose lease has expired is revoked and reassigned by the next
process to start"); I introduced the loop-owner claim without one. That asymmetry is
mine, it is HIGH, and it is not a reason to weaken the nesting refusal.

The epoch fence already makes reclaim SAFE: `GoalLoopOwnerFinished` requires the exact
live epoch, so once a successor claims epoch 2, a resurrected epoch-1 owner's
termination is refused. The missing half is only the liveness evidence that says the
old owner is gone — which is what a lease is. Fixing next, mirroring the ledger's
proven design rather than inventing a second one.
