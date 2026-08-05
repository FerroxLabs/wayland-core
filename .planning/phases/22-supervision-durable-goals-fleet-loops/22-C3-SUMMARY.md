---
lane: lane/22-c3-goal
criterion: "22-C3 — Direct, ForgeFlows, Fleet, Council and Anvil terminate through one canonical Goal transition with no nested verification/retry owner"
grade-22-C3: PARTIAL — materially advanced (production paths 1/5 → 5/5, exactly-once proven live); opt-in attachment and a Linux-only proof remain
engines-converged: 5/5 production paths, live-proven on the shipped 0.12.25 release binary (was 1/5 at base)
nested-owner-proof: 5 concept queries each carrying a known-positive; 1 production writer; 0 raw-terminate callers; compile gate FALSIFIED red; live refusal after kill -9; exactly-once counted 1 finished / 2 claimed
new-finding: the four unwired adapters DID NOT TYPECHECK against their production entry points (Anvil ForgeError, Council anyhow::Error) — proof the wiring was never attempted; and the crucible verb has TWO council routes, the DEFAULT of which bypassed the Goal in my own first wiring
fence-exposure: "crates/wcore-cli/src/main.rs +61/-0, one contiguous block, 0 removal lines; crates/wcore-cli/src/lib.rs untouched (vs 861d1b1a)"
status: complete
---

# Phase 22 Criterion 3 — five engines, one transition, and an honest ceiling

Lane `lane/22-c3-goal`. Base `861d1b1a` (captured once, quoted everywhere). Built and
proven on `hetzner-dsm`; live legs against `wayland-core 0.12.25` **release**, Linux.

> **Criterion 3, verbatim:** *"Direct, ForgeFlows, Fleet, Council, and Anvil terminate
> through one canonical Goal transition with no nested verification/retry owner."*

> **On the earlier lane.** `lane/22-c3` ran 2026-07-28/29 and its summary occupied this
> filename. It is preserved unedited in git at `aa60fc4b` (with `8894f443`, `d82ac121`,
> `0e33c08d`), its working notes are preserved verbatim as **Part I** of
> `22-C3-NOTES.md`, and its substance is carried in §0 below. Nothing was discarded.

---

## 0. My brief's premise was stale, and I checked before building

My dispatch brief said C3 "remains untouched", "was never attempted", and that "five
engines still return five types". **The first two were false at my base.** A prior lane
built the adapter surface and graded itself **PARTIAL**.
`22-PHASE-VERDICT.md`'s `UPDATE — 2026-07-27` predates that lane and was never re-run, so
the verdict and the `GOAL-*` ledger row both still assert a fact the tree falsifies.

I verified against **source**, not against the prior summary — a summary can itself be
advertised-but-dead — and found five adapters, 880 lines, present.

**The third clause is still true and I am not claiming otherwise.** The five engines DO
still return `ClimbOutcome`, `CouncilRunResult`, `WorkflowRunError`, a caller-chosen `T`
and nothing. **I did not change one engine signature.** What changed is that all five now
have a **production path** to one canonical Goal transition.

So my job was not to build the surface; it was to close the PARTIAL.

## 1. The gap I was actually closing, measured

```
$ /usr/bin/grep -rn "from_direct\|from_forgeflows\|from_fleet\|from_council\|from_anvil" \
    crates --include='*.rs' | /usr/bin/grep -v "^crates/wcore-agent/src/goal/strategy.rs"
```
Known-positive in the same sweep: `fn main` in `wcore-cli/src` → **23** (instrument alive).

**24 hits. 23 in `tests/goal_strategy_test.rs`. Exactly ONE production caller** —
`goal_cmd.rs`, `from_fleet`. Four of five adapters were reachable only from a test binary:
**advertised-but-dead**, the class this programme has recorded ten times.

**Root cause, and it was not "nobody got round to it".** `goal open` **hard-coded
`GoalStrategy::Fleet`**, so the durable record could never authorize another owner and
`GoalLoop::claim::<S>` refused every non-Fleet strategy with `StrategyMismatch`. Zero
occurrences of `GoalStrategy::{Direct,ForgeFlows,Council,Anvil}` existed anywhere in
`wcore-cli`. **The product could not express four of its five strategies.**

## 2. NEW FINDING — the adapters did not typecheck against production

The strongest evidence the four were never wired, and the compiler produced it:

| Adapter | Took | Shipped entry point returns |
|---|---|---|
| `from_anvil` | `Result<&ClimbOutcome, &EngineError>` | `drive_climb_full` → `Result<_, **ForgeError**>` — a disjoint type |
| `from_council` | `Result<&CouncilRunResult, &CouncilError>` | `drive_council` → **`anyhow::Result`**`<CouncilRunResult>` |

A missing caller is consistent with neglect. **A signature that cannot accept what the
production entry point returns is proof the wiring was never attempted**, because
attempting it fails to compile on the first try. The suite could be fully green while the
product path did not typecheck.

**What I refused to do:** squeeze `ForgeError::NoGate` into `EngineError::Builder`, or
flatten every council error into `Blocked`. Both compile. The module already names that
anti-pattern and already had the answer (`FleetOutcome::DriverFailed`), so I mirrored it:
`AnvilOutcome` and `CouncilRunOutcome`, each with a typed arm plus `DriverFailed`.
`CouncilRunOutcome::from_anyhow` **downcasts**, so a wrapped `CouncilError` still reaches
its exact category — `Unpriced` survives, the one carrier the 22-02 census said the lifted
taxonomy had to add.

## 3. What landed

Each engine's **existing shipped verb** gained an opt-in Goal attachment that wraps its
**real** invocation in `GoalLoop::run_<strategy>`. I deliberately did **not** add a
`goal drive` verb: that would be a parallel demo path, proving only that the demo reaches
the transition — the same defect one level up.

| Engine | Shipped verb | Attachment |
|---|---|---|
| Fleet | `goal run --terminate` | already present at base |
| ForgeFlows | `workflow run <NAME>` | `--goal` / `--goal-journal` flags |
| Anvil | `forge "<task>"` | `--goal` / `--goal-journal` flags |
| Council | `crucible "<task>"` | env (`WAYLAND_GOAL_ID` + `WAYLAND_GOAL_JOURNAL`) |
| Direct | headless `wayland-core "<prompt>"` | env, same pair |

Plus `goal open --strategy {direct,forge-flows,fleet,council,anvil}`, defaulting to `fleet`
so every existing invocation and 22-03's kill/restart proof are byte-for-byte unchanged.

**Why env for two of them.** `CrucibleArgs` and the Direct prompt path are assembled
field-by-field in `main.rs` — the shared multi-lane fence. Flags there would have cost four
non-contiguous fence edits. The env pair is the mechanism this codebase **already** uses to
hand a Goal's identity to a child process (`goal_cmd::ENV_GOAL`), not an invention. Flags
win where present; env is a fallback, never an override.

## 4. Live — all five engines, shipped release binary

**No credential was used and none was needed.** A local canned OpenAI-compatible endpoint
on `127.0.0.1:18422` (unique port; many lanes live) serves model tokens — every layer under
test is production code. Harness liveness carried a known-positive AND a negative control
(the same probe against a dead port → `rc=7`). Config isolated via `XDG_CONFIG_HOME`
rather than editing `/root/.config`, which other live lanes share.

| # | Engine | Canonical transition observed |
|---|---|---|
| 1 | Fleet | `PartiallyCompleted { completed: 3, failed: 0 }`, `cursor_seq=Some(22)` |
| 2 | Direct | `NeedsEscalation`, `cursor_seq=Some(2)` |
| 3 | ForgeFlows | `PartiallyCompleted { completed: 0, failed: 1 }`, `cursor_seq=Some(2)` |
| 4 | Council | `PartiallyCompleted { completed: 1, failed: 0 }`, `cursor_seq=Some(2)` |
| 5 | Anvil | `Blocked { reason: "probe builder failed: …" }`, `cursor_seq=Some(2)` |

Every line is the product's own `GOAL: canonical_transition …`, printed from the **durable
record** read back after the fact — not from the value the adapter returned.

Direct's `NeedsEscalation` is the documented mapping for a *completed* Direct run: Direct
has no verification owner, so `SelfChecked` would assert checks that never ran. Anvil
exercised **both** new arms — `ForgeFailed` (a `ForgeError` that `from_anvil` previously
could not accept at all) and `Climbed`.

## 5. The live run found a defect 3079 green tests could not

**The Council leg exited 0, printed a fused council answer, and terminated NO Goal.**

Cause: the `crucible` verb has **two** routes to a council — `run_crucible_auto` →
`drive_council` (which I had attached) and a **manual** path → `run_council` directly
(which I had not). A plain `[crucible] proposers = [...]` config — **the default** — takes
the second. My own first wiring was therefore advertised-but-dead: flag present, code
compiled, tests green, default path terminating nothing.

**It was caught only because the gate asserts the transition LINE, not the exit status**,
which was `0`. This is the brief's *"a 'sole path' that had three"* arriving inside my own
construction, and it is the single strongest argument here for driving the real binary.

Fixed with `CouncilRunOutcome::RanManual`, carrying a bare `CouncilOutcome` rather than
fabricating an empty `AssemblyPlan` the assembler never produced.

## 6. Nested-owner proof — a known-negative, so every query carries a known-positive

| Q | Query | Known-positive | Result |
|---|---|---|---|
| Q1 | `finish_loop_owner\|GoalLoopOwnerFinished` | 6 hits | **ONE production writer** (`strategy.rs`, in `GoalLoop::finish`); `finish_loop_owner` is `pub(crate)` |
| Q2 | concept: `retry\|retries\|re_?try\|attempt\|reattempt\|backoff\|re_?run\|rerun\|re_?verify\|max_attempts\|max_iterations\|escalat` | `attempt` in 237 files | 40 hits in `goal/`, **none a Goal-level retry owner** — all are `attempts:` payload fields or task-level ledger bookkeeping |
| Q3 | `\.run_(direct\|forgeflows\|fleet\|council\|anvil)\(` | `run_fleet` 5 hits | **exactly FIVE production call sites, one per engine** (the two others are inside `#[cfg(test)]`, which begins at line 774 — `grep -v /tests/` does NOT exclude inline test modules) |
| Q4 | `\.terminate\(\|\.terminate_verified\(` | — | **ZERO** Goal-related production callers; all 7 hits are process/job termination in other crates |
| Q5 | the `compile_fail` retry-wrapper doctest | — | **FALSIFIED** (below) |

**Q5 is load-bearing and the rest corroborate.** A `compile_fail` doctest passes when the
snippet fails to compile for *any* reason, including a typo I introduced while editing it.
So I removed the retry loop — one variable, `for outcome in outcomes` →
`if let Some(outcome) = outcomes.first()` — and the gate went **RED**:

```
test ... from_anvil (line 512) - compile fail ... FAILED
Test compiled successfully, but it's marked `compile_fail`.
NEGATIVE_CONTROL_RC=101
```
Restored; `git diff --stat` empty. **The retry loop is the only reason that gate is green**,
so the borrow-checker refusal of a nested retry owner is real: `LoopOwner` is neither
`Clone` nor `Copy` and every adapter takes it by value.

**What I am NOT claiming:** that nothing in Core ever retries. Fleet retries tasks, Anvil
climbs to `max_iterations`, ForgeFlows re-attempts validation, Council re-solicits
proposals. The claim is narrower and is exactly what the queries test: **once a Goal has a
loop owner, exactly one owner owns its verification/retry, and no second owner can produce,
repeat or override its termination.**

## 7. Kill mid-flight — terminates EXACTLY ONCE

`kill -9` on the **process group** mid-wave, workers holding `/bin/sleep 40`, lease 120s.

```
descendants BEFORE kill: 9      AFTER: 0
transitions emitted by the KILLED process: 0
```

**A. Restart while the lease is live — the nested owner is REFUSED, from the product:**
```
EARLY_RC=1
goal g-k2 did not terminate: invalid journal state transition:
  loop owner Fleet (epoch 1) is already live; a nested loop owner is refused
```
The Goal is left non-terminal and **resumable** — a refusal, not a corruption.

**B. Past the lease — supersede, complete, terminate:** `PartiallyCompleted {completed: 4,
failed: 0}`, `cursor_seq=Some(48)`, `LATE_RC=0`.

**C. Exactly once, counted off the product's own projection (`goal stream`):**

| Record | Count |
|---|---|
| `loop_owner_claimed` | **2** — two epochs: the killed owner, then its successor |
| `loop_owner_finished` | **1** — one termination across a `kill -9` and THREE `goal run` invocations |

**D.** A third run reports `recovery=already-terminal … resumable=false`, dispatches
nothing, and `loop_owner_finished` **stays 1**. Duplicate termination is as wrong as none;
there was no duplicate.

**The counter is not stuck.** Known-positive: terminated goal → 1. Known-negative: a goal
that never ran → 0. The product's `--expect` gate is falsifiable at a point, not merely in
one direction: `--expect 8→rc=1, 9→rc=1, 10→rc=0, 11→rc=1, 12→rc=1`.

## 8. One-variable negative control — a bypassing engine REDDENS

Gate: a Council run must terminate its Goal canonically
(`canonical_transition_lines == 1 && loop_owner_finished == 1`). Same command, same config,
same binary; **one variable**, `WAYLAND_GOAL_ID` set or unset.

```
CONTROL  (attached): crucible rc=0   lines=1  finished=1   GATE=GREEN  rc=0
NEGATIVE (bypass):   crucible rc=0   lines=0  finished=0   GATE=RED    rc=1
```
The bypassing run **exited 0 and produced a real council answer**. So the gate can fail,
and it fails exactly when an engine reaches its own terminal without the Goal.

## 9. What is NOT done — the part that matters

1. **Attachment is opt-in.** An engine invoked with no Goal still runs and terminates its
   own way. That is **convention, not construction**, and §8 is the measurement of it, not
   a workaround for it. Making an engine structurally incapable of terminating outside a
   Goal still requires threading a token through five entry points.
2. **I did NOT re-derive the earlier lane's "≥40 test call sites" ceiling — because I did
   not take that route.** The opt-in wrapper changed zero engine signatures, so no engine
   test was touched. That settles that 5/5 production paths are reachable *without* a mass
   test edit; it does **not** settle the structural-impossibility claim, and I leave that
   where the earlier lane left it rather than pretending my route closed it.
3. **Anvil never reached `Verified` live.** That needs a real model to produce a candidate
   passing a real gate, and credentials are Sean-reserved. The `Verified` path is covered
   by tests only. I am not claiming it was live-proven.
4. **Linux only.** No Windows leg for any of this.
5. **F05 rows 2 (mid-flight monitor) and 4 (learned policy) remain `runtime path
   unwired`.** I did not touch them. Stated plainly rather than left ambiguous.
6. **The taxonomy gaps the earlier lane reported are unchanged** — no "completed but
   unchecked" category, and `PartiallyCompleted{failed:0}` still *reads* as partial failure
   though its payload is exact.
7. **ForgeFlows' and Anvil's live legs terminated on failure branches**, because the canned
   endpoint does not reach sub-agent spawners. The termination path is what was under
   proof and it is proven; the success-branch terminal categories for those two are
   test-covered, not live-covered.

## 10. Gates — real numbers, read back

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | rc=0, **0 bytes** (first run was RED, 8537 bytes; fixed) |
| `cargo clippy -p wcore-agent -p wcore-cli --all-targets --all-features -- -D warnings` | **rc=0, 0 error lines** — status captured **without a pipe** |
| `cargo check --workspace --all-targets` | rc=0 |
| `cargo nextest run -p wcore-agent` | **3079 run, 3079 passed**, 11 skipped |
| `cargo nextest run -p wcore-agent --test goal_strategy_test` | **17 run, 17 passed, 0 skipped** |
| `cargo test --doc -p wcore-agent -- goal::strategy` | **2 passed, 0 failed** (nextest does NOT run doctests) |
| `cargo nextest run -p wcore-cli` | 2336 run, 2335 passed, **1 timed out — PRE-EXISTING** |

**The wcore-cli red is classified, not assumed.**
`remedy_advertisements::advertised_tool_names_resolve_to_a_real_tool`, `TRY 2 TMT` at
60.004s — a genuine timeout, not the `exec failed` fd-exhaustion class, so the brief's
reclassification escape does not apply. Run alone at HEAD: timed out. Run alone at **BASE
`861d1b1a`: identical, `TRY 2 TMT [60.004s]`.** Pre-existing; the worktree was restored to
HEAD afterwards and the restore verified in the same log.

## 11. Four defects in my OWN instruments, all repaired in-lane (§6b-ii)

The brief is explicit that documenting an instrument defect without repairing it is a
defect you have agreed to keep. All four were repaired, not merely noted:

1. **zsh ate an unquoted glob** — `--include=*.rs` → `no matches found`. Quoted thereafter.
2. **`grep -c "Checking wcore-cli"` returned 0 on a SUCCESSFUL build** — `--all-targets`
   prints **`Compiling`**. Repaired to `grep -cE "(Checking|Compiling) wcore-cli v"` with
   the required **three** assertions: known-positive → 1, known-negative → 0, and **the old
   matcher on the known-positive → 0, i.e. it would have missed it**. That third assertion
   is the only one proving the repair does anything.
3. **`grep -c` on the journal returned 0 because the journal is binary-framed.** Every
   count in §7 therefore comes from the product verb `goal stream`, not raw `grep`.
4. **A pipe stole clippy's exit status** — `cargo clippy … | tail; echo RC=$?` printed
   `CLIPPY_RC=0` on a RED run (the brief's very first self-passing class). Repaired by
   redirecting to a file; the repaired instrument immediately reported `CLIPPY_RC=101`, a
   red the broken one had called green.

I also introduced and fixed a **real** clippy regression: declaring a const between
`#[allow(clippy::too_many_arguments)]` and `drive_climb_full` **detached the attribute**,
unsuppressing a lint that predates this lane.

## 12. Grade

**PARTIAL.** Not FAILED, and materially further than base:

- production paths went **1/5 → 5/5**, live on the shipped release binary;
- every Goal termination goes through **one** transition — 1 writer, 5 call sites (one per
  engine), **0** production callers of the raw terminate path;
- the no-nested-owner half is enforced at compile time (**gate falsified red**) and at the
  durable boundary (**refusal observed live after `kill -9`**);
- restart safety holds with **exactly one** termination, counted, with the counter shown
  able to read 0 and the gate shown able to red.

**Not PASSED**, on one honest reading I will not argue away: attachment is **opt-in**, so
an engine run with no Goal is unenforced (§9.1). There is a defensible narrower reading —
the criterion constrains how a *Goal* terminates, and every Goal termination does go
through the one transition, while a Goal-less run terminates no Goal at all. I record that
reading because it is real, but **I do not grade on it**, because the phase's purpose is
supervision, and an unsupervised run is precisely what supervision must not be optional
about.

## 13. For the orchestrator to serialize

- **Shared fence:** `main.rs` **+61 / −0**, ONE contiguous additive block, **0 removal
  lines**, no reordering or renames. `lib.rs` **untouched**. Measured with
  `/usr/bin/git diff --numstat 861d1b1a -- …` against the **SHA**, never a branch name.
- **`crates/wcore-cli/src/goal_cmd.rs` is shared with lane `22-wire`** — merge this lane
  after `22-wire` if both are outstanding.
- **Two adapter signatures changed** (`from_anvil`, `from_council`) and
  `crates/wcore-agent/tests/goal_strategy_test.rs` was **modified** — 7 call sites adapted
  mechanically to the new signatures. **No assertion was changed, nothing was `#[ignore]`d,
  re-gated or deleted.** Disclosed because the earlier lane's summary claimed "no existing
  test was modified", and that is no longer true of this file.
- **No new `SessionEvent` or `ClimbOutcome` variants from this lane.** No contract fixture
  was regenerated and **`wcore-contract generate` was NOT run**. No wire change is
  requested, so **no fenced seam request is owed**.
- `FORGE_REQUIRED_STABILITY` is now public in `anvil::forge`, read by both the forge and
  the CLI, so the `Verified` bar cannot drift between them.
- **`22-PHASE-VERDICT.md` and the `GOAL-*` COMPETITIVE-LEDGER row are both stale** (§0) and
  need re-grading by whoever owns them. I did not edit either — they are not my artifacts.
