# 22-02 Task 1 — The five loop owners, measured against the source

Tree: `2ecdfdf54ff7fda920eec7d068337006e5da4ee4`. Every line below was re-taken
from the file at execution time. Where the plan's own truths disagreed with the
file, the file wins and the disagreement is recorded in §7.

---

## 1. Direct — the agent engine's own turn loop

| Property | Measured value |
|---|---|
| Source | `crates/wcore-agent/src/engine.rs` |
| Terminal shapes it can produce | `AgentError` (public error type), a turn-limit stop governed by `--max-turns`, a cancellation, and a normal completion. The engine has **no named terminal enum**: "done" is the absence of a further turn. |
| Retry owner | The provider attempt layer (`provider_attempt_*` journal events) and the turn loop bound `max_turns`. |
| Verification owner | None. Direct produces no verdict about its own output. |
| Can it be nested inside another? | Yes — every other engine spawns Direct runs beneath itself. Direct is the leaf. |
| Durable record today | `SessionEvent::TurnStarted` / `TurnCommitted` / `TurnFailed` / `TurnCancelled` in the F12 chain. |

**What a naive canonical mapping would lose:** nothing, because Direct has the
least information of the five. Direct is the *floor* of the taxonomy, not its
shape. A taxonomy designed from Direct outward would be a boolean.

---

## 2. ForgeFlows — `WorkflowRunner`

| Property | Measured value |
|---|---|
| Source | `crates/wcore-agent/src/orchestration/workflow/runner.rs` (108 KB), `.../limits.rs` |
| Terminal shapes | `WorkflowRunError` variants measured at runner.rs:440–491: `Cycle(Vec<String>)`, `MissingPrompt(String)`, `StageFailed { stage, message, partial }`, `SchemaValidationFailed { stage, attempts, message, partial }`, `NodeNotInGraph(String)`, `DispatchBudgetExceeded { limit, attempted, partial }` — plus the success value `WorkflowRunResult`. |
| Retry owner (bound) | `const MAX_SCHEMA_RETRIES: usize = 2` at runner.rs:416, i.e. up to `1 + 2 = 3` dispatches per schema-bearing node (runner.rs:415, and the loop at `for attempt in 0..=MAX_SCHEMA_RETRIES` at runner.rs:600 and again at runner.rs:1367). |
| Second, independent bound | `DispatchBudget` (limits.rs:103) charged through `try_charge` (limits.rs:118) and `try_charge_n` (limits.rs:131) against `MAX_TOTAL_DISPATCHES`. Its doc comment states it counts "every dispatch on every path (single, fan-out, fleet, pipeline item, schema retry, loop iteration)". |
| Verification owner | The schema validator, per node. It is an *output-shape* check, not an outcome check. |
| Can it be nested? | It nests things: fan-out, fleet and pipeline dispatch all charge the same budget. |
| Partial-result discipline | Three error variants carry `partial: Box<WorkflowRunResult>` so completed work is never discarded. |

**What a naive canonical mapping would lose:** the distinction between
`SchemaValidationFailed` (the model kept producing the wrong shape — a *quality*
exhaustion) and `DispatchBudgetExceeded` (the DoS backstop fired — a *resource*
exhaustion). Collapsing both into "failed" destroys the only signal that tells an
operator whether to fix the prompt or raise the budget. And every one of the three
partial-carrying variants would lose its partial payload if the canonical
transition only carried a category.

**Load-bearing observation for F22C:** `DispatchBudget`'s own doc says it already
counts schema retries. So ForgeFlows internally is already a *single* budget owner
across two loops. That is the exact construction the canonical Goal transition
needs, and it exists here as precedent rather than as an invention.

---

## 3. Fleet — `FleetDispatcher`

| Property | Measured value |
|---|---|
| Source | `crates/wcore-swarm/src/fleet.rs` |
| Terminal shapes | Success: `ShardSummary { shard_id, agent_count, successes, failures, payload }` per shard, reduced by a `FleetReducer<T>` to a caller-chosen `T`. Failure: `FleetError::Topology(TopologyError)`, `FleetError::Shard { shard_id, source: MeshError }`, `FleetError::Timeout(Duration)`. |
| Retry owner (bound) | **None.** Fleet has no retry. Its bounds are `shard_size` (default `DEFAULT_SHARD_SIZE = 10`) and `shard_timeout: Duration`, both fields of `FleetDispatcher`. |
| Verification owner | None. `default_shard_reducer` counts `r.succeeded` booleans off `AgentReport`; nothing checks whether the work was right. |
| Can it be nested? | Yes — the workflow runner dispatches fleet waves through the same `DispatchBudget`. |
| Terminal type is generic | The final fleet result type `T` is chosen by the caller's reducer. **There is no fixed Fleet terminal type at all.** |

**What a naive canonical mapping would lose:** the per-shard `successes`/`failures`
split. A fleet run of 100 agents where 97 succeeded is not "failed", and it is not
"succeeded" either. This is the clearest case in the census for an explicit
*partially-checked* category rather than a boolean.

**Finding (recorded, not repaired here):** because `T` is caller-chosen, Fleet
cannot be mapped onto a canonical transition by its return type. The adapter must
bind at the `ShardSummary` level, before the caller's reducer collapses it. Any
design that adapts `T` is adapting whatever the caller felt like returning.

---

## 4. Council (Crucible) — `run_council` / `drive_council`

| Property | Measured value |
|---|---|
| Source | `crates/wcore-agent/src/orchestration/council/run.rs`, `.../driver.rs` |
| Terminal shapes | Success: `CouncilOutcome { final_text, proposals, skipped: Vec<SkippedProposer>, chosen_from, spend }` (run.rs:130). Driver level: `CouncilRunResult::Direct { spec, text }` / `::Council { plan, outcome }` / `::Cancelled` (driver.rs:44). Failure: `CouncilError::NoResolver`, `::InsufficientProposals { got, need }`, `::OverBudget { estimated_usd, cap_usd }`, `::UnpriceableRoster`, `::DailyBudgetExhausted { spent_usd, cap_usd }` (run.rs:145). |
| Retry owner (bound) | **None.** Proposers run once, in parallel. `min_proposers` is an admission threshold, not a retry. |
| Verification owner | The aggregator — a fenced, read-only fusion pass. It is a *model* judge. |
| Can it be nested? | Yes. |
| Unpriced outcome is already explicit | `CouncilError::UnpriceableRoster` — "a council member has no verified price, so a budget ceiling cannot be certified — refuse rather than run against an undercounted estimate". |

**What a naive canonical mapping would lose:** `skipped: Vec<SkippedProposer>`.
A council answer fused from 2 of 5 proposers because 3 were keyless is not the
same artifact as a unanimous one, and the difference is invisible in `final_text`.

**Load-bearing observation for the taxonomy:** Council's verification owner is an
LLM aggregator. Under the F20-GATE-02 discipline that Phase 22 inherits, a
council outcome can therefore **never** reach a `verified` terminal state. That
is a taxonomy constraint discovered from the source, and it is the single most
important census result for Success Criterion 3.

---

## 5. Anvil — `run_climb`

| Property | Measured value |
|---|---|
| Source | `crates/wcore-agent/src/orchestration/anvil/engine.rs`, `.../mod.rs`, `.../climb.rs` |
| Terminal shapes | `ClimbOutcome { terminal: TerminalState, stamp, checks_passed, checks_total, iterations, valve_fires, winner, best_worktree, landing: Option<LandingReport> }` (engine.rs:246). Abort-before-terminal: `EngineError::Builder(String)` / `::Gate(String)` (engine.rs:208). Landing: `LandingReport::Landed{..} / Conflict / Incomplete / RolledBack / RecoveryRequired / Failed` (engine.rs:294). Stall evidence: `StallReport` (engine.rs:182). |
| Retry owner (bound) | `ClimbParams::max_iterations` (hard cap, probe counts as 1) plus `stall_after` consecutive identical fail-hashes, plus a whole-climb `deadline: Option<Instant>`. |
| Verification owner | A **real executable gate** producing `GateReport`, evaluated against `Acceptance` / `RejectReason` / `FailSet` / `Severity` in `climb.rs`. This is host-observed deterministic evidence, not a model claim. |
| Can it be nested? | It nests: builders are spawned children; the valve is one frontier turn. |

### THE CENTRAL FINDING OF THIS CENSUS

`crates/wcore-agent/src/orchestration/anvil/mod.rs:52` already defines a terminal
taxonomy, and its in-source comment calls it "the COMPLETE enum (spec §6.5).
Every climb ends in exactly one of these... There is no silent fourth exit."

```
pub enum TerminalState {
    Verified,            // ONLY a real Tier-1 gate passing with stability
    CriteriaChecked,     // Tier-2, user-confirmed derived criteria
    SelfChecked,         // Tier-3, self-generated checks — "correlated evidence, not truth"
    NeedsEscalation,     // some checks remain uncracked
    Blocked(String),     // could not proceed, stated reason
    Cancelled,
    TimedOut,
    PermissionDenied,
    CrashedRecovered,
    Superseded,
}
```

with `TerminalState::is_verified()` documented as deliberately "a single, tight
predicate".

This is **already** the taxonomy Phase 22 Success Criterion 3 asks for:

- It keeps partially-checked outcomes as explicit categories (`CriteriaChecked`,
  `SelfChecked`, `NeedsEscalation`) instead of rounding them to success/failure.
- It reserves `Verified` for host-observed deterministic gate evidence, which is
  exactly the F20-GATE-02 property 22-01's Test 4 is meant to enforce.
- It already carries `CrashedRecovered` and `Superseded`, which are precisely the
  restart-safety categories 22-03's Fleet ledger needs.

**Recommendation carried into 22-01 Task 3 and 22-02 Task 2:** the canonical Goal
terminal taxonomy should be this taxonomy, LIFTED to `wcore-types` so all five
strategies and the protocol crate can name it, with Anvil re-exporting rather than
duplicating. Inventing a sixth vocabulary beside it would be the exact "parallel
lifecycle" PROJECT.md forbids, and it would leave the strongest existing evidence
discipline in the codebase stranded behind an adapter summary.

What the lifted taxonomy still needs, measured from the other four engines:

| Needed carrier | Because |
|---|---|
| an `Unpriced` category | `CouncilError::UnpriceableRoster` refuses to run rather than guessing a ceiling; folding that into `Blocked(String)` loses the fact that the run never started for a *pricing* reason |
| a partial payload slot | three `WorkflowRunError` variants carry `partial: Box<WorkflowRunResult>`, and Fleet carries `successes`/`failures` per shard |
| a resource-vs-quality exhaustion distinction | `DispatchBudgetExceeded` and `SchemaValidationFailed` are both "ran out of attempts" and mean opposite things |

---

## 6. Nesting: is the problem real today?

Measured, and yes:

- `DispatchBudget` (workflow) counts fleet dispatches and pipeline items, so a
  Fleet run inside ForgeFlows has **two** bounds: the fleet's `shard_timeout` and
  the workflow's dispatch budget. Neither knows about the other's units.
- Anvil's `max_iterations` × the workflow's `MAX_SCHEMA_RETRIES` would be a bound
  multiplied by a bound if a climb were placed inside a schema-bearing node. F22C
  forbids exactly this ("no generic retry wrapper around an Anvil climb").
- Council has no retry, so it is the only one of the five that cannot multiply a
  bound. It can still be nested and consume budget.

**Loop-owner claim needed:** at most one of {workflow schema-retry, Anvil climb,
fleet shard} may be the retry owner for a given Goal, and it must be recorded on
the durable Goal record rather than inferred per turn.

## 7. Where the plan's own truths disagreed with the file

| Plan 22-02 truth | File |
|---|---|
| "Council is `run.rs` and `driver.rs`, producing `CouncilOutcome` ... `CouncilRunResult`, or `CouncilError`" | Confirmed exactly. |
| "Fleet ... producing `ShardSummary` or `FleetError`" | Incomplete. The *fleet-level* result is a caller-chosen generic `T` produced by `FleetReducer<T>`; `ShardSummary` is the per-shard intermediate. An adapter written against the plan's sentence would bind to the wrong level. |
| "Anvil ... producing `ClimbOutcome`, `LandingReport`, `StallReport` or `EngineError`" | Confirmed, but the plan does not mention that `ClimbOutcome.terminal` is **already** a ten-variant canonical terminal enum. That omission is the difference between "define a new taxonomy" and "lift the existing one", and it changes the whole design. |
| "five terminal vocabularies ... Success Criterion 3 requires ONE" | Confirmed as a count, but one of the five is already fit for purpose. |

## 8. What was NOT measured

- No runtime measurement of nesting cost was taken (no live climb-inside-workflow
  run). The nesting analysis above is structural, from bounds declared in source.
- `intent.rs` was not re-read; the plan's constraint that it stays task-shape
  routing is carried forward unexamined.
