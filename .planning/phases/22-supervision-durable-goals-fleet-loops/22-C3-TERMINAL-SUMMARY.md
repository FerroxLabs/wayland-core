---
lane: lane/22-c3-terminal
criterion: "22-C3 — Direct, ForgeFlows, Fleet, Council and Anvil terminate through one canonical Goal transition with no nested verification/retry owner"
grade-half-A-one-canonical-transition: "ADVANCED — the last representable bypass is closed at the durable boundary for all five owners and any sixth; engine invocation with NO Goal remains opt-in and is NOT closed"
grade-half-B-no-nested-owner: "CLOSED, but not by this lane — pre-existing, re-verified only (2 compile_fail doctests executed)"
enforcement-achieved: "durable-boundary impossibility, uniform over 5/5 owners + any sixth; NOT compile-time impossibility for un-goaled invocation (0/5)"
falsified: "old shape restored -> 5/5 per-engine cases + the sweep RED; failure output shows the bypass write COMMITTED (RecoveryCursor journal_sequence Some(1))"
gates: "fmt 0 bytes; workspace check rc=0; clippy rc=0 on every changed target; wcore-agent 3212/3212; doctests 2 passed; wcore-cli 2407/2408 with 1 PRE-EXISTING timeout proven at base"
live: "shipped 0.12.25 release binary, Linux — canonical_transition observed, 1 claimed / 1 finished, exactly-once across a re-run, counter shown able to read 0"
status: complete
---

# Phase 22 Criterion 3 — closing the bypass that was still representable

Lane `lane/22-c3-terminal`. Base `5be910561f688c75d39492e7b982d6e100772a64`, asserted
against `git ls-remote` before any work and quoted everywhere. Built and proven on
`hetzner-dsm`; live legs against `wayland-core 0.12.25` **release**, Linux.

> **Criterion 3, verbatim:** *"Direct, ForgeFlows, Fleet, Council, and Anvil terminate
> through one canonical Goal transition with no nested verification/retry owner."*

---

## 0. My brief's measurements — one held, one was a dead instrument, one was stale

The brief asked me to re-measure three claims before acting. **This is the third
consecutive lane on this criterion to be dispatched against a stale premise**, and the
staleness has now compounded: the ledger row is stale, *and its own 2026-07-29 correction
is stale too*.

| # | Brief claim | Verdict at my base | Evidence |
|---|---|---|---|
| 1 | `GoalTerminalState` has consumers **only** in `goal/{ledger,kernel}.rs` and `session_journal/model.rs` | **FALSE** | 24 files across 4 crates (`wcore-types`, `wcore-protocol` ×3, `wcore-agent` src ×8 + tests ×7 + examples ×1, `wcore-cli` ×3) |
| 2 | Grep under `orchestration/` returns zero hits | **TRUE, and the instrument is dead** | 0 hits confirmed; known-positive `ClimbOutcome` in the same directory → **21** hits, so the grep is alive. The adapter was never *built* under `orchestration/` — it is `goal/strategy.rs`. This falsifier reports FAILED forever, including after the criterion closes. |
| 3 | Anvil still returns `ClimbOutcome` at `engine.rs:246,346` | **TRUE in substance, stale in line numbers** | struct at `:247`; returned at `:362` and `:614` |

**And the finding that reframed the lane.** `crates/wcore-agent/src/goal/strategy.rs`
(41,803 bytes), `22-C3-SUMMARY.md` and `22-C3-NOTES.md` are **all present in integration at
my base**. The ledger's correction states the work "is NOT in the integration branch" at
`ef1d97be`; integration has since moved to `5be91056` and it is. So:

- the row's *"no lane has attempted it"* — **false**;
- the correction's *"not in the integration branch"* — **also now false**.

**Two lanes had already worked C3.** `lane/22-c3` built the adapter surface; `lane/22-c3-goal`
wired all five production paths and live-proved them, self-grading PARTIAL. I verified
against **source**, not against either summary.

## 1. What actually remained, and it was not what the brief described

The brief asked me to "build the adapter surface over the five owners" and said "the five
engines still return five types". The surface exists; the five engines do still return five
types, and **I did not change one engine signature either** — for reasons §4 measures.

The real remaining gap was the prior lane's own honest ceiling (§9.1): attachment is
**opt-in** — *"convention, not construction"*. So I went looking for what was still
**representable**, and found something narrower and sharper than "the CLI has an `if let`".

## 2. THE FINDING — the bypass was in the reducer, and it was justified in a comment

`GoalKernel::terminate` is **public**, and `SessionEvent::GoalTerminated` was refused only
while a loop owner was live:

```rust
if let Some(owner) = &goal.loop_owner { return Err(...); }
// A Goal that never claimed an owner is still terminable this way —
// but by definition no engine ran it, so there was no loop owner to
// be canonical about.
```

**That premise is false exactly when attachment is opt-in, i.e. always.** An engine can run
a Goal to completion, never claim, and then record `SelfChecked` — a full engine verdict —
straight down the plain path. Nothing above stops it, because no claim was ever taken.

This is precisely the brief's test — *"a sixth engine added tomorrow cannot bypass it"* — and
it **failed**: a sixth engine is in that state **by default**, because claiming is the thing
it would have to opt into. The bypass was not hypothetical; §6 shows the write committing.

The cross-audit panel (codex 5.6-sol / gemini 3.1-pro / kimi K3 + an internal adversarial
pass) was **unanimous** that totalizing the CLI's `resolve()` is *not* structural closure —
a future caller can simply not call it — and unanimous that `engine.run` is the wrong
boundary for Direct. Two members wanted capability receivers across all five; kimi argued
the conversion boundary was already sealed and the real fix was to delete the non-canonical
*termination* paths. **The internal adversarial pass sided with the minority, and
measurement settled it**: the non-canonical termination path was real, public, and one edit
from closed.

## 3. What landed

**`GoalTerminalState::requires_loop_owner()`** (`wcore-types`) — an exhaustive split of the
taxonomy into *engine-produced* (a claim about work that ran: `Verified`, `CriteriaChecked`,
`SelfChecked`, `PartiallyCompleted`, `Exhausted`, `NeedsEscalation`, `Unpriced`, `Blocked`,
`TimedOut`) and *control-plane* (lifecycle facts needing no engine: `Cancelled`,
`PermissionDenied`, `CrashedRecovered`, `Superseded`, `AuthorityUnreconstructable`).
**There is deliberately no wildcard arm** — a sixth category fails to compile until someone
decides which side it is on, the same device `strategy_tag_name` uses for a sixth strategy.

**The reducer refuses an engine verdict on the plain path unconditionally.** An
engine-produced category now reaches the journal through `GoalLoopOwnerFinished` — hence
through one of the five adapters, each consuming a `LoopOwner` by value — or it does not
reach the journal at all.

`Verified` is deliberately exempt *here* and refused **more** strongly elsewhere
(`terminate` rejects it outright; `terminate_verified` demands a `VerifiedTerminal`
obtainable only from a `HostGateObservation` with no deserialization route). Folding it in
would tighten nothing and would break that older gate's only route.

**No variant was stranded.** All 9 engine-produced categories are reachable through the
adapters — measured, not assumed, before the refusal was written.

## 4. Enforcement level achieved — stated per engine, and it is NOT what the brief's binary offers

The brief asked for *compile-time impossibility*, with *a failing test* as fallback, per
engine. What I achieved is a **third thing**, and I am naming it rather than forcing it into
either box:

| Owner | Un-goaled **invocation** blocked at compile time? | Engine verdict on a Goal without its owner? |
|---|---|---|
| Direct | **No** | **Cannot be represented in the durable record** + failing test |
| ForgeFlows | **No** | same |
| Fleet | **No** | same |
| Council | **No** | same |
| Anvil | **No** | same |

**Durable-boundary impossibility is not weaker than a compile error — it is orthogonal, and
in one respect stronger:** it holds against a hand-built journal record, which no type system
can reach. It is the mechanism this architecture already uses (`SessionJournal::append`
refuses every `Goal*` variant for the same reason).

**What I did NOT achieve, plainly: 0/5 compile-time closure of un-goaled invocation.** I
measured the cost before declining it — `drive_climb_full` 5 call sites, `drive_council` 3,
but `WorkflowRunner::run` **~34** and Direct's `engine.run` **thousands**. Two panel members
recommended it; gemini warned that *mixed* enforcement is worse than uniform because it
breeds false confidence. **I agree with that warning, and it is the reason I chose the
uniform durable-boundary mechanism over threading tokens into the two cheap engines only.**
A 2-of-5 compile-time story would have been exactly the false-confidence shape.

## 5. Per-owner gate — three assertions each, none shared across the set

`crates/wcore-agent/tests/goal_no_bypass_test.rs`. Every one of the five carries all three:

1. **known-positive** — the engine's real outcome through its real adapter reaches the
   durable record (catches an over-broad refusal that breaks the product);
2. **known-negative** — the same verdict on the plain path with no owner is refused, and the
   Goal is left live and **resumable**, not half-applied;
3. **the old shape would have missed it** — `loop_owner` is asserted `None` at the moment of
   refusal. That is *precisely* the condition the previous guard tested, so the old reducer
   would have accepted every one of these writes.

All five refusals, verbatim from the product's own error, `7 tests run: 7 passed, 0 skipped`:

```
DIRECT_REFUSAL=invalid journal state transition: goal direct-bypass: NeedsEscalation is an engine verdict and requires the goal's loop owner; claim one and terminate through the canonical strategy transition
FORGEFLOWS_REFUSAL=... goal forgeflows-bypass: PartiallyCompleted { completed: 1, failed: 1 } is an engine verdict ...
FLEET_REFUSAL=... goal fleet-bypass: PartiallyCompleted { completed: 11, failed: 3 } is an engine verdict ...
COUNCIL_REFUSAL=... goal council-bypass: Blocked { reason: "no proposer answered" } is an engine verdict ...
ANVIL_REFUSAL=... goal anvil-bypass: SelfChecked is an engine verdict ...
```

Plus a sweep over **every** engine-produced category (a rule keyed on a hand-written variant
list could pass all five samples and still leak a sixth), and an assertion that an operator
`Cancelled` still works — or the refusal would have broken cancellation.

## 6. Falsification — and it shows the bypass COMMITTING

One variable: the new refusal neutralized to `if false && …`, which restores the old shape
exactly (the loop-owner-live guard remains).

```
NEGATIVE_CONTROL_RC=100
Summary: 7 tests run: 1 passed, 6 failed, 0 skipped
  FAILED: direct_ / forgeflows_ / fleet_ / council_ / anvil_terminates_through_its_owner_and_nowhere_else
  FAILED: no_engine_produced_category_is_reachable_without_an_owner
```

**5/5 per-engine cases red**, plus the sweep. The one survivor is
`every_strategy_has_a_no_bypass_case`, a compile-time set-completeness check that does not
touch the reducer — correct that it is unaffected.

The failure text is the strongest single line in this lane, because it is not "the assertion
did not fire" — it is the **cursor of a committed journal write**:

```
Direct: NeedsEscalation must not be reachable without the goal's loop owner:
  RecoveryCursor { journal_sequence: Some(1), journal_digest: "2324fded…" }
ForgeFlows: PartiallyCompleted { completed: 1, failed: 1 } must not be reachable ...:
  RecoveryCursor { journal_sequence: Some(1), journal_digest: "ee28c51b…" }
```

The old shape **accepted the bypass and durably recorded it**. Reducer restored with
`git checkout -- <one path>` (permitted — moves no ref, touches no other lane); restore
verified by re-running the gate green (`7 passed`) and by `grep -c "if false &&"` → **0**.

## 7. Five existing tests went red — adapted, not weakened

The change made **5 existing tests fail**, every one of them terminating a Goal with an
engine verdict on the raw path. That they were pre-existing tests, not ones I wrote, is
itself evidence the bypass was on live code paths.

**Nothing was `#[ignore]`d, re-gated, deleted, or relaxed.** Each was routed through the
sanctioned path — claim the one loop owner, hand the engine's real outcome to its adapter.
`multi_day_journey` and `goal_kernel`'s crash walk are Anvil-authorized Goals, so
`from_anvil` produces the identical terminal; the wire test's `PartiallyCompleted{11,3}` now
travels from a **real** `ShardSummary` through `from_fleet`, which is strictly stronger — if
the adapter ever rounded 11-and-3, this now catches it and before it could not have.

**The kernel round-trip needed care and did not lose coverage.** Its engine half moved to a
crate-internal test over the canonical transition (`finish_loop_owner` is `pub(crate)`
precisely so a test cannot reopen the bypass — the module already says so), and the
integration test *gained* the refusal assertion over every engine shape. Coverage is a
**superset** of the original, not a partition.

## 8. Gates — real numbers, read back from unproxied tools

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | rc=0, **0 bytes** |
| `cargo check --workspace --all-targets` | **rc=0** (workspace-wide, never `-p` — changing a shared type is exactly what breaks downstream exhaustive matches) |
| `cargo clippy -D warnings`, each changed target | **rc=0** ×7 (`wcore-types`, `wcore-agent --lib`, and all 5 touched test targets) |
| `cargo nextest run -p wcore-agent` | **3212 run, 3212 passed**, 0 failed, 6 skipped (base 3204; +8 from this lane) |
| `cargo nextest run -p wcore-agent --test goal_no_bypass_test` | **7 run, 7 passed, 0 skipped**; `0 ignored; 0 measured; 6 filtered out` |
| `cargo test --doc -p wcore-agent -- goal::strategy` | **2 passed, 0 failed** (nextest does NOT run doctests; count read back so a zero-test pass is visible) |
| `cargo nextest run -p wcore-cli` | 2408 run, **2407 passed, 1 timed out — PRE-EXISTING** |

**Two reds classified rather than assumed, both proven at MY base `5be91056`:**

- `wcore-cli::remedy_advertisements::advertised_tool_names_resolve_to_a_real_tool`,
  `TRY 2 TMT [60.004s]`. Run **alone at base**: `1 test run: 0 passed, 1 timed out`,
  identical. Not mine.
- `clippy -D warnings` on the full `-p wcore-agent --all-targets` is **rc=101** from **4
  `needless_borrows_for_generic_args` hits in `tests/user_model_identity_wire.rs`** — a file
  outside my diff (`git diff --name-only $BASE HEAD` lists 8 paths; that is not one). Proven
  at base in a detached worktree: **`BASE_CLIPPY_RC=101`, same hits.** Left alone per the
  scope-boundary rule; named here rather than fixed.

## 9. Live — the shipped release binary

`wayland-core 0.12.25` release, Linux, real on-disk journal. **No credential was used and
none was needed** — the Fleet worker is `/bin/echo`, so every layer under test is production
code. Evidence written to `/tmp/22c3term-live` (lane-unique, per §6a-ii).

```
GOAL: opened goal=g-live strategy=Fleet iterations=1 envelope=wayland-core-goal-fleet/v1
GOAL: canonical_transition strategy=fleet
      terminal=Terminated { terminal: PartiallyCompleted { completed: 1, failed: 0 } }
      cursor_seq=Some(10)
loop_owner_claimed  1
loop_owner_finished 1
```

Re-run against the now-terminal Goal: `recovery=already-terminal … resumable=false`,
dispatches nothing, `loop_owner_finished` **stays 1** — duplicate termination is as wrong as
none, and there was none.

**The counter is not stuck.** Known-negative in the same journal: a Goal opened and never
run reads **0**.

This leg's purpose is a non-regression proof — that the refusal is not over-broad and the
real product still reaches the canonical transition end to end. **It is not a live proof of
the refusal itself**, and I am not claiming it is: the shipped CLI exposes no raw-terminate
verb, so the bypass is not reachable from the binary at all. The refusal is proven by tests
that drive the **real kernel and a real on-disk journal**, not mocks.

## 10. What is NOT done

1. **Attachment is still opt-in.** An engine invoked with no Goal still runs and terminates
   its own internal type. I closed the case where a Goal *exists* and an engine sidesteps its
   canonical transition; I did **not** make having a Goal mandatory. The prior lane's §9.1
   stands, narrowed but not removed.
2. **0/5 compile-time impossibility for un-goaled invocation** — §4, with the measured cost
   and the reason I judged a 2-of-5 version worse than none.
3. **Half B (no nested verification/retry owner) was not advanced by this lane.** It was
   already closed by construction and I only **re-verified** it (2 `compile_fail` doctests
   executed). I did not re-derive the prior lane's falsification of it; I am not claiming its
   evidence as mine.
4. **In-crate reach.** `GoalKernel::terminate` stays `pub`. The refusal is on the *value*, not
   the *caller*, so it binds `wcore-agent`'s own `orchestration/` too — which is what makes it
   uniform — but a `pub(super)` tightening of the API surface is still available and I did not
   take it (it would have broken integration tests that legitimately prove refusals).
5. **Linux only.** No Windows or macOS leg.
6. **Anvil never reached `Verified` live** — needs a real model and a real gate; credentials
   are Sean-reserved. Test-covered only. Unchanged from the prior lane.
7. **Out of scope, found not fixed:** the 4 pre-existing clippy hits in
   `tests/user_model_identity_wire.rs` (§8).

## 11. For the orchestrator to serialize

- **Shared fence: `crates/wcore-cli/src/lib.rs` and `main.rs` are BOTH UNTOUCHED.** `git diff
  --name-only $BASE HEAD` lists 8 paths and neither is among them. No fence exposure at all.
- **`crates/wcore-types/src/goal.rs` gains a public method** (`requires_loop_owner`) — purely
  additive, no variant added, no signature changed. Any lane touching `GoalTerminalState`
  should merge after this one.
- **`crates/wcore-agent/src/session_journal/reducer.rs`** — one added refusal block in the
  existing `GoalTerminated` arm; a lane touching that arm should merge after this one.
- **No `SessionEvent` variant, no protocol change, no contract fixture regenerated.**
  `wcore-contract generate` was **NOT** run. **No fenced seam request is owed.**
- **The `22-C3` ledger row AND its 2026-07-29 correction are both stale** (§0) and need
  re-grading by whoever owns them. I did not edit either — they are not my artifacts. The
  correction's own repaired falsifier is still the right one to use.
