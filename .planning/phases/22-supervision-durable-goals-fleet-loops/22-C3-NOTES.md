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
