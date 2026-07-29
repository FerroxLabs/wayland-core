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

---

## M8 — close-out

HIGH from M7 FIXED (`84228e92` lease, `bc047d56` `--lease` wiring), and the fix was
re-proven live: kill -9 → 9 descendants → 0 → restart REFUSED while the lease was live
(`EARLY_RC=1`) → past the lease, superseded, 4/4 completed, terminated canonically at
epoch 2 (`LATE_RC=0`, `cursor_seq=Some(48)`), effects 4/4 with the gate falsified
(`--expect 99` → rc=1).

`wcore-cli` shows 2 failures (`release_binary_smoke` /
`plugin_discovery_e2e`, both `capabilities.browser_suite`). **Measured at BASE
`0b16f867`: `4 tests run: 2 passed, 2 failed` — the same two.** Pre-existing; this lane
touches no plugin-discovery code.

`cargo check --workspace --all-targets` rc=0 / ERRCOUNT=0 — run deliberately because new
`SessionEvent` variants are exactly the change class that breaks downstream exhaustive
matches while a `-p` check stays green.

Shared fence `crates/wcore-cli/src/{lib,main}.rs`: **untouched**, verified by
`git diff $BASE --stat` against the captured SHA, never against the branch name.

Verdict written to `22-C3-SUMMARY.md`: **Criterion 3 = PARTIAL**. The construction is
real and structurally enforced over the Goal lifecycle; four of five engines have no
product path through it, and engine invocation outside a Goal remains convention.

= = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = = =

# PART II — lane `lane/22-c3-goal`, 2026-07-29

Everything above this line was written by the earlier lane `lane/22-c3` and is
**preserved unedited**. Everything below is mine. Base `861d1b1a` (which already
contains all of the above). Append-and-commit after every measurement.

---

## §0. FIRST FINDING — my dispatch brief's premise is stale, in two layers

My brief states C3 "remains untouched", "was never attempted", and that "five engines
still return five types". **All three were true when written; none is true at my base.**
I establish this before building, because building against a stale premise would have
produced a second copy of an existing construction.

**Layer 1 — the phase verdict is stale.** `22-PHASE-VERDICT.md` §`UPDATE — 2026-07-27`
re-grades C3 **FAILED, unchanged**, stating "No lane attempted 22-02 Task 3." Written
2026-07-27 by lane `lane/22-wire`.

**Layer 2 — a C3 lane ran AFTER that update and nobody re-graded.** Unproxied `git log`:

```
$ /usr/bin/git log --format='%h %ad %s' --date=short -- .../22-C3-NOTES.md .../22-C3-SUMMARY.md
aa60fc4b 2026-07-29 docs(22-c3): SUMMARY with an honest PARTIAL grade on Criterion 3
8894f443 2026-07-29 docs(22-c3): gate results, the live transcript, and a HIGH I found in my own construction
d82ac121 2026-07-28 docs(22-c3): record the closed-back-door design and the full terminal mapping
0e33c08d 2026-07-28 docs(22-c3): record the Criterion 3 enforcement-ceiling determination before building
```

Ordering: verdict UPDATE (07-27) → C3 lane builds the adapter surface (07-28/29) → **no
re-grade**. The verdict AND the `GOAL-*` COMPETITIVE-LEDGER row both still assert a fact
the tree falsifies.

**Layer 3 — confirmed against SOURCE, not against the summary.** A summary can itself be
advertised-but-dead (ten recorded instances on this programme), so I checked the tree:

```
$ /usr/bin/grep -n "pub fn from_" crates/wcore-agent/src/goal/strategy.rs
262:    pub fn from_direct(owner: LoopOwner<DirectTag>, outcome: DirectOutcome<'_>) -> Self {
300:    pub fn from_forgeflows(
347:    pub fn from_fleet(owner: LoopOwner<FleetTag>, outcome: FleetOutcome<'_>) -> Self {
378:    pub fn from_council(
464:    pub fn from_anvil(
```

Five adapters, 880 lines, present. **The construction exists.** "Five engines return five
types" remains literally true of the *engines' own signatures* — the adapter surface that
converges them onto `StrategyTermination` is what exists.

### What this changes about my job

I am **not** building the adapter surface. I am closing the **PARTIAL**. The earlier
lane's own §6 names the gap and that gap is my work:

1. **Four of five engines have no PRODUCT path.** Only Fleet is reachable via a shipped
   verb (`wayland-core goal run --terminate`). Direct, ForgeFlows, Council, Anvil
   terminate canonically **in tests only**. Against my brief's own instruction — "prove
   each engine's *production* path reaches it, not just that a function exists" — this is
   exactly the **advertised-but-dead** class in a new costume.
2. **An engine invoked outside any Goal is unenforced** — convention, not construction.
   The earlier lane measured the reason (≥40 existing test call sites; 22-02 Task 3's
   `<done>` forbids modifying existing tests) and called it a Sean-level scope call.
3. Linux only; no Windows leg.

**Grading position, stated BEFORE I build so it cannot be retrofitted:** item 1 is
squarely mine and is the whole difference between a construction and a product property.
Item 2 I will **re-derive rather than inherit** — an inherited impossibility claim is
precisely what this programme has been wrong about three times.

---

## §1. What I still have to establish

- [ ] Re-derive item 2's ≥40-call-site claim myself. Do NOT inherit it.
- [ ] Give the four unwired engines a product path, or prove it impossible with the call
      sites that make it so.
- [ ] **Nested-owner proof.** A known-negative is self-passing on a dead instrument. Must
      search the CONCEPT (retry / re-run / attempt / backoff / verify-again / max_*),
      state the query, and carry a known-positive in the SAME invocation.
- [ ] Kill-mid-flight: the goal terminates **exactly once**. The earlier lane proved claim
      durability; termination *cardinality* is a different assertion and is mine.
- [ ] A one-variable negative control that reddens.
- [ ] State plainly whether F05 rows 2 (mid-flight monitor) and 4 (learned policy) become
      reachable. Default answer is "they remain unwired" unless I actually wire them.

## §2. Log

- **T+0** — worktree verified `.../lane-22-c3-goal`, branch `lane/22-c3-goal`, HEAD
  `861d1b1a`. Confirmed NOT the dirty `/Users/seandonahoe/dev/waylandcore` checkout.
- **T+9** — §0 established and committed. Premise stale; job re-scoped from "build the
  adapter surface" to "close the PARTIAL".

---

## §3. MEASURED — the advertised-but-dead gap, with the query that found it

**Query** (unproxied, glob quoted — an earlier unquoted run was eaten by zsh with
`no matches found: --include=*.rs`, which is the §3b-i trap arriving in my own hands):

```
$ /usr/bin/grep -rn "from_direct\|from_forgeflows\|from_fleet\|from_council\|from_anvil" \
    crates --include='*.rs' | /usr/bin/grep -v "^crates/wcore-agent/src/goal/strategy.rs"
```

**Known-positive in the same style**, to prove the instrument is alive:
`/usr/bin/grep -rn "fn main" crates/wcore-cli/src --include='*.rs' | wc -l` → **23**.

**Result: 24 hits. 23 of them are `crates/wcore-agent/tests/goal_strategy_test.rs`.
Exactly ONE is production — `crates/wcore-cli/src/goal_cmd.rs:532`, `from_fleet`.**

That is the finding stated precisely: **four of the five adapters have zero production
callers.** `from_direct`, `from_forgeflows`, `from_council`, `from_anvil` are reachable
only from a test binary. A caller of the shipped product cannot make four of the five
engines terminate through the canonical transition, because no shipped code path calls
those adapters at all.

This is the **advertised-but-dead** class (ten recorded instances on this programme, four
found on 2026-07-29). The construction is real, the type-level enforcement is real, and
**four fifths of it is dead code from the product's point of view.**

## §4. Where each engine's REAL production path lives

Measured, not assumed:

| Engine | Production entry point | Shipped verb |
|---|---|---|
| Fleet | `FleetDispatcher` via `GoalFleetDriver` | `goal run --terminate` — **already canonical** |
| ForgeFlows | `WorkflowRunner::run` — `crates/wcore-cli/src/workflow.rs:241` | `workflow run <NAME>` |
| Council | `drive_council` — `crates/wcore-cli/src/crucible.rs:459` | `crucible` |
| Anvil | `run_climb` via `forge.rs:821` — `crates/wcore-cli/src/anvil.rs:42 run_forge` | `anvil forge` |
| Direct | `Engine::run` | main agent loop — entry still to be located |

So the four unwired engines **do** have shipped verbs. They simply do not route their
termination through the Goal. That makes the gap closable without inventing a demo path.

## §5. Design decision (Decide-don't-park), taken before building

**Rejected: a new `goal drive` verb that re-implements each engine's invocation.** It
would be a parallel demo path — a fifth way to start an engine, proving only that the
demo reaches the transition. That is the advertised-but-dead defect rebuilt one level up.

**Chosen: attach the Goal to each engine's EXISTING production verb.** Each of
`workflow run`, `crucible`, `anvil forge` and the Direct loop gains an opt-in
`--goal <id> --journal <path>` pair. When supplied, that verb's real engine invocation
runs INSIDE `GoalLoop::run_<strategy>` and terminates through its adapter. When absent,
the path is byte-for-byte what it is today.

This is the pattern `goal run --terminate` already established, adopted for the same
stated reason: an always-on termination would break 22-03's kill/restart proof, which
re-enters the same verb. Opt-in keeps every existing invocation and every existing test
unchanged — which also means I do **not** have to edit the ≥40 test call sites the earlier
lane measured as blocking, because I am not changing any engine signature.

**What this does and does not buy.** It makes all five engines terminate through one
canonical transition *when driven under a Goal*, from the product, on the engines' real
entry points. It does **not** make an engine invoked with no Goal impossible — that
remains convention, and §6 of the earlier SUMMARY is still correct about it. I will
re-derive that ceiling myself rather than inherit it (§1).

## §6. The live-proof problem, and how I intend to solve it without a credential

Four of the five engines need an LLM provider to reach termination. I have no credential
and credentials are Sean-reserved. Measured: there is **no offline/mock provider** in
production code — `StubProvider` is `#[cfg(test)]` in `spawner.rs`, `FixtureProvider` is
`wcore-evolve`'s and is a `ParaphraseProvider`, not an `LlmProvider`.

**Plan: a local canned-response HTTP endpoint on hetzner, pointed at by `base_url`.**
Providers are HTTP + `ProviderCompat`, so an OpenAI-compatible server on `127.0.0.1:<port>`
with a dummy key exercises the **real** provider code, the **real** engine loop and the
**real** termination path — only the model's tokens are canned. No secret involved, so
this does not touch the credential rule at all. Unique port per §"many lanes are live".

**Stated honestly up front:** this proves the *termination path*, not model quality. If
it turns out an engine cannot be driven to termination this way, I will say so and show
the call sites, not invent an exit (Honesty rule: no "termination state 4").

---

## §7. NEW FINDING — the adapters were typed against engine internals, not the
## shipped verbs. Two of five did not even compile against production.

This is the strongest evidence I have that the four adapters had never been production
wired, and I did not have to argue for it — the compiler produced it.

**Anvil.** `from_anvil` took `Result<&ClimbOutcome, &EngineError>`. The shipped forge
entry point is `drive_climb_full`, which returns `Result<ClimbOutcome, ForgeError>`.
`ForgeError` (`NoGate`, `Lease`, `Worktree`, `GateUnrunnable`, `Receipt`, `Disabled`) is a
disjoint type from `EngineError` (`Builder`, `Gate`). **There was no way to call the Anvil
adapter from the Anvil verb.**

**Council.** `from_council` took `Result<&CouncilRunResult, &CouncilError>`. The shipped
`drive_council` returns `anyhow::Result<CouncilRunResult>`. Compiler, verbatim:

```
error[E0308]: mismatched types
   --> crates/wcore-cli/src/crucible.rs:510:74
    |  StrategyTermination::from_council(owner, Err(&error))
    |                                                ^^^^^^ expected `&CouncilError`, found `&Error`
```

**Why this matters more than the call-site count.** A missing caller is consistent with
"nobody got round to it". A signature that *cannot accept* what the production entry point
returns is proof the wiring was never attempted, because attempting it fails to compile on
the first try. The construction was verified against the engines' internal APIs and the
tests exercised those same internal APIs, so the test suite could be fully green while the
product path did not typecheck.

**What I did NOT do:** squeeze `ForgeError::NoGate` into `EngineError::Builder`, or flatten
every council error into `Blocked`. Both would have compiled. The module itself already
names that anti-pattern — *"squeezing that into `FleetError::Timeout` to satisfy a
signature would be a fabricated terminal"* — and it already had the answer:
`FleetOutcome::DriverFailed`. I mirrored it: `AnvilOutcome` and `CouncilRunOutcome`, each
with a typed arm and a `DriverFailed` arm. `CouncilRunOutcome::from_anyhow` **downcasts**,
so a wrapped `CouncilError` still lands on its exact category — `Unpriced` survives, which
is the one carrier the 22-02 census said the lifted taxonomy had to add.

## §8. A defect in MY OWN instrument, found and repaired in-lane (§6b-ii)

I gated each build on `grep -c "Checking wcore-cli"`. It returned **0** on a build that had
succeeded — because `cargo check --all-targets` prints **`Compiling wcore-cli`** for the
binary target, not `Checking`. So my gate reported "the crate was not built" while it had
been built cleanly, and, worse, the same matcher returning 0 is what a genuinely skipped
build looks like. **A known-negative that fires for free — §3b-i, arriving in my own hands
for the second time this lane** (the first was zsh eating `--include=*.rs`).

§6b-ii is explicit that writing this up and moving on is not a fix, so I repaired the
matcher to `grep -cE "(Checking|Compiling) wcore-cli v"` and gave it the required **three**
assertions, run on hetzner:

```
A1 known-positive, repaired matcher (expect >0): 1
A2 known-negative, repaired matcher (expect  0): 0
A3 known-positive, OLD broken matcher (expect 0 = would have MISSED it): 0
--- repaired matcher on the real build log (expect 1): 1
```

**A3 is the one that matters** — it shows the old matcher scored 0 on a log that genuinely
contains the build, so the repair changes an outcome rather than decorating a passing test.

## §9. Log (continued)

- **T+40** — `goal open --strategy` landed; ForgeFlows attached to `workflow run`;
  `cargo check -p wcore-cli --all-targets` rc=0, errors=0.
- **T+70** — Anvil + Council attached. Two adapter signature changes forced by §7.
  Test call sites adapted mechanically (no assertion changed, nothing `#[ignore]`d,
  nothing deleted) — disclosed because the earlier lane's SUMMARY claimed "no existing
  test was modified" and that is no longer true of this file.
- **T+75** — instrument defect §8 found and repaired.

