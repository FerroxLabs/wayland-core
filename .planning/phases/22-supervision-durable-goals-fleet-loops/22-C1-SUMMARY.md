# 22-C1 — durable Goals on the protocol and TUI surfaces

**Lane:** `lane/22-c1` · **HEAD:** `d5a77d02` · **Base:** `8bcb052b` (`plan/f20-unified-audit-repair`)
**Build/test host:** `hetzner-dsm`, `/root/wayland-22-c1` (branch `hz/22-c1`) and
`/root/wayland-22-c1-base` (detached at base, for the differential).

**Verdict: BOTH SURFACES LANDED. The criterion is CLOSER, not MET — and the gap is named
precisely in §6.** One thing this lane could not do is fenced to another lane, and one
pre-existing red is left red on purpose.

---

## 1. What the criterion said, and what was actually true at base

`22-C1` (`ROADMAP.md`, restated `INV-21-23.md:89-96`): *a user sees and controls the same
Goal/child/task/wait/log/cursor/terminal state whether they use the CLI, the TUI, or a host
over the protocol — and Core emits the serialized fixtures Desktop replays at D2.*

Measured in my worktree at `8bcb052b` before any edit:

```
grep -ri 'goal' crates/wcore-protocol/src/ | wc -l   -> 0
grep -ri 'goal' crates/wcore-cli/src/tui/  | wc -l   -> 1   (statusline/mod.rs, unrelated prose)
crates/wcore-cli/src/tui/surfaces/goals.rs           -> does not exist
```

One surface of three, exactly as the inventory said.

## 2. What landed

| Layer | File | What it is |
|---|---|---|
| protocol | `crates/wcore-protocol/src/goal.rs` (**new**) | the host-observable Goal projection |
| protocol | `crates/wcore-protocol/src/events.rs` | `ProtocolEvent::GoalSnapshot`, `ProtocolEvent::GoalTransition` — **additive** |
| protocol | `crates/wcore-protocol/src/contract/{spec,generate}.rs` | `EVENT_SPECS` 49→51, `PRODUCER_EVENT_TYPES` 57→59, `SOURCE_INPUTS` +`goal.rs`, capability `durable_goals_v1`, `CONTRACT_MINOR` 8→9 |
| agent | `crates/wcore-agent/src/goal/wire.rs` (**new**) | the conversion, the drift guard, `goal_stream` |
| cli | `crates/wcore-cli/src/goal_cmd.rs` | `wayland-core goal stream` — the live producer path |
| tui | `crates/wcore-cli/src/tui/app.rs` | `App.goals`, `App.goal_last_transition`, cleared on `/new` |
| tui | `crates/wcore-cli/src/tui/protocol_bridge.rs` | the two ingest arms + `goal_status_summary` |
| tui | `crates/wcore-cli/src/tui/widgets/statusbar.rs` | the Goal segment a user actually sees |

**The wire reuses `wcore_types::goal` directly** — `GoalStrategy`, `GoalTerminalState`,
`LoopPolicy`, `WaitKind`. No second Goal vocabulary was minted. Only the three shapes with no
`wcore-types` home are mirrored (lifecycle, authority record, task ledger), because they live in
`wcore-agent` and `wcore-protocol` cannot depend on it.

**No authority route is opened.** `GoalAuthorityWire` is descriptive. The only function that
turns a durable record into an effective envelope is `GoalAuthorityRecord::reconstruct`, it
lives in `wcore-agent`, and it takes `GoalAuthorityRecord` — not the wire type. No signature
anywhere accepts `GoalAuthorityWire` as authority.

## 3. The design decision that the live run proved was necessary

Each `goal_transition` reports the lifecycle AFTER it. I fold the journal prefix through
`replay_state` — **the** reducer — rather than deriving the lifecycle from the transition's
name, on the grounds that `RunResumed` and `LoopOwnerClaimed` do not determine one.

The live stream settles it. Rows 1 and 2 below both report `opened`:

| # | transition | cursor seq | lifecycle after |
|---|---|---|---|
| 0 | opened | 0 | opened |
| 1 | **run_resumed** | 3 | **opened** |
| 2 | **loop_owner_claimed** | 4 | **opened** |
| 3 | iteration_started | 5 | running |
| 4 | iteration_started | 11 | running |
| 5 | loop_owner_finished | 17 | terminated |
| 6 | *(goal_snapshot)* | 17 | terminated / `partially_completed{completed:2,failed:0}` |

A name-derived projection would have written `running` for both and been **wrong twice in a
seven-event stream**. This is the one design call I could not have reasoned my way to; the
measurement made it.

## 4. Live evidence — the shipped binary, with counts

Build identity asserted BEFORE any measurement:
`wayland-core 0.12.25 (source d5a77d023941fc6ca483a6932f8a38ae15e1251b)` — this lane's HEAD.
Caller-generated nonce `22c1final-1785290238-16790`. Script + captures:
`22-C1-EVIDENCE/live-drive.sh`, `22-C1-EVIDENCE/live/`.

A Goal was opened, given two tasks with a real dependency, and driven through the **real
`FleetDispatcher`** with `--terminate`, so the chain being projected was written by the product:

```
GOAL: run_complete waves=2 iterations=2 completed=2 delivered=2
GOAL: canonical_transition strategy=fleet terminal=Terminated { terminal:
      PartiallyCompleted { completed: 2, failed: 0 } } cursor_seq=Some(17)
```

Then the new surface:

```
$ wayland-core goal stream --journal … --goal g-22c1final-…
GOAL-STREAM: events=7 transitions=6 snapshots=1
stdout bytes=3209   stderr bytes=82
lines=7  goal_snapshot=1  goal_transition=6  valid_json_lines=7  invalid=0
```

The snapshot carries the task ledger with dependencies and outcomes intact:

```
task build    completed epoch=1 attempts=1 deps=[]        outcome={"state":"self_checked"}
task publish  completed epoch=1 attempts=1 deps=["build"] outcome={"state":"self_checked"}
iterations 2/4   resume_count 1   loop_owner_epochs 1
```

**Every gate in the drive was falsified in the same run.**

| gate | falsification | result |
|---|---|---|
| `--expect 999` | must refuse | **rc=1** "expected 999 goal events, emitted 7" |
| `--expect 7` | must pass | **rc=0** |
| `--goal g-does-not-exist` | must refuse, not print an empty Goal | **rc=1**, stdout **0 bytes** |
| replay determinism | second stream byte-identical | sha256 `402bc25e…` on both |

Every capture byte-counted. `${PIPESTATUS[0]}` avoided entirely — each rc read from `$?`
immediately after the command, never across a pipe.

## 5. Gate results — real numbers, and how they were read

All on `hetzner-dsm` at `d5a77d02` unless stated. Graded with
`22-C1-EVIDENCE/read-cargo-result.py` (see §7 — the naive reading of these logs is wrong).

| suite | result |
|---|---|
| `cargo check --workspace --all-targets` | **0 errors** |
| `cargo clippy -p wcore-protocol -p wcore-agent -p wcore-cli --all-targets -- -D warnings` | **rc=0** (had been **rc=101** on my own code minutes earlier — the gate is falsifiable and was falsified) |
| `cargo fmt --all -- --check` (Mac) | **rc=0** |
| `wcore-protocol --lib` | **125 passed / 0 failed** (incl. 5 new, named in output) |
| **`wcore-protocol --test golden_v0_1_21`** | **22 passed / 0 failed** |
| `wcore-protocol --test host_decoder_contract` | **31 passed / 0 failed** |
| `wcore-protocol --test desktop_contract_adversarial` | **17 passed / 0 failed** |
| `wcore-protocol --test recovery_protocol` | **14 passed / 0 failed** |
| `wcore-agent --test goal_protocol_wire_test` (new) | **8 passed / 0 failed** |
| `wcore-agent --test goal_kernel_test` | **10 passed / 0 failed** |
| `wcore-agent --test goal_strategy_test` | **17 passed / 0 failed** |
| `wcore-agent --test goal_fleet_ledger_test` | **11 passed / 0 failed** |
| `wcore-cli --lib` | **1844 passed / 0 failed / 1 ignored** |
| **`wcore-protocol --test desktop_contract_corpus`** | **13 passed / 2 failed — RED, see §6** |

**`golden_v0_1_21` at 22/22 is the load-bearing green.** It pins the wire contract. If I had
reshaped `ready` or any existing event, that is where it would have gone red. It did not.

### The one RED, and the differential that says whose it is

I ran the same test at **base**, in a separate detached worktree, to establish attribution:

| | base `8bcb052b` | HEAD `d5a77d02` |
|---|---|---|
| result | **FAILED. 14 passed; 1 failed** | **FAILED. 13 passed; 2 failed** |
| `missing` | `[]` | `[events/goal_snapshot.json, events/goal_transition.json]` |
| `drifted` | 5 artifacts | the same 5, plus 3 `schema/*.json` |
| second failure | — | `manifest_pins_generator_and_all_three_digests`: `left: 8  right: 9` (my deliberate minor bump) |

**This gate did not go green→red in my lane. It went red→differently-red.** A regeneration was
already owed at base by three other lanes' edits to `SOURCE_INPUTS`
(`wcore-agent/src/bootstrap.rs`, `wcore-agent/src/output/protocol_sink.rs`,
`wcore-cli/src/main.rs`). Corroborated independently by `22-C1-EVIDENCE/contract-drift-probe.py`,
which re-implements the generator's digests in Python and reproduces its `schema_digest`
**exactly** while `source_inputs_digest` mismatched at base.

**I am not reporting this green and I did not weaken the test to make it green.** It is red, the
reason is named to the artifact, and the fix is §6.

## 6. FENCED SEAM REQUEST — Desktop producer contract

<!-- ============================ SEAM REQUEST ============================ -->
```text
SEAM REQUEST — SR-22-C1 — Desktop producer contract: TWO ADDITIVE EVENTS
STATUS:  OPEN. NOT PERFORMED BY THIS LANE. LANE-BRIEF §0 forbids `wcore-contract generate`.
OWNER:   the lane that owns the single regeneration over the merged tree, + Sean for the
         Desktop co-pin. NOT mergeable by re-running the generator on this branch alone.
RAISED:  2026-07-29 by lane/22-c1 at d5a77d02.

--------------------------------------------------------------------------------
1. EXACTLY WHICH EVENTS WERE ADDED  (the two, and only these two)
--------------------------------------------------------------------------------
  goal_snapshot     required: type, goal_version, session_id, goal_id, cursor,
                              state_digest, goal
                    criticality: Observational    correlation: goal_id_and_cursor
                    capability:  durable_goals_v1
                    fixture:     contracts/desktop/v1/events/goal_snapshot.json

  goal_transition   required: type, goal_version, session_id, goal_id, cursor,
                              transition, lifecycle
                    criticality: Observational    correlation: goal_id_and_cursor
                    capability:  durable_goals_v1
                    fixture:     contracts/desktop/v1/events/goal_transition.json

  ZERO commands were added. ZERO existing events or commands were reshaped.
  `ready` is byte-untouched. golden_v0_1_21 is 22/22 GREEN, which is the evidence.

--------------------------------------------------------------------------------
2. EVERYTHING ELSE THIS MOVES  (so the regenerating lane can diff against it)
--------------------------------------------------------------------------------
  EVENT_SPECS               49 -> 51
  PRODUCER_EVENT_TYPES      57 -> 59
  SOURCE_INPUTS             40 -> 41   (+ crates/wcore-protocol/src/goal.rs)
  contract_capabilities()   + durable_goals_v1 = ShapeOnly
  CONTRACT_MINOR            8 -> 9                      <-- VETOABLE, see §4
  GENERATOR_VERSION         unchanged (wcore-desktop-contract-gen/11)
  desktop_contract_corpus.rs count assertion 49 -> 51 (already updated in-lane)

--------------------------------------------------------------------------------
3. THE REGENERATION WAS ALREADY OWED BEFORE THIS LANE  (measured, both directions)
--------------------------------------------------------------------------------
  At base 8bcb052b, desktop_contract_corpus ALREADY FAILED: 14 passed / 1 failed,
  drifted=[adversarial/events/{fixture,schema,version}-mismatch.jsonl,
           events/ready.json, manifest.json], missing=[], "run `wcore-contract generate`".

  Cause: 3 of 40 SOURCE_INPUTS changed since the last authorized re-stamp 5f74d559
  (2026-07-28) — wcore-agent/src/bootstrap.rs, wcore-agent/src/output/protocol_sink.rs,
  wcore-cli/src/main.rs. All other lanes' work.

  So this request does not CREATE a regeneration; it adds two named fixtures and one
  version bump to one that has to happen anyway. Fold it into that single pass.

--------------------------------------------------------------------------------
4. THE ONE JUDGEMENT CALL, FLAGGED FOR VETO
--------------------------------------------------------------------------------
  I bumped CONTRACT_MINOR 8 -> 9. An additive event set IS a minor bump, and leaving it
  at 8 while the event count moves 49 -> 51 would be a dishonest version: a host pinned
  to "1.8" would negotiate successfully against a producer emitting two events 1.8 never
  described. If the regenerating lane or Sean prefers to batch the bump with other
  additive work, revert `CONTRACT_MINOR` to 8 — ONE constant in
  crates/wcore-protocol/src/contract/generate.rs:24 — and nothing else in this lane
  changes. I have no strong stake in the number, only in it not silently staying still.

--------------------------------------------------------------------------------
5. WHY THIS IS A REAL CONTRACT CHANGE, NOT A DIGEST RE-STAMP
--------------------------------------------------------------------------------
  SEAM-REQUESTS/CONTRACT-DIGEST-RESTAMP.md is explicitly a bookkeeping re-stamp:
  "No event added or removed ... the wire *shape* is identical."  THIS ONE IS NOT THAT.
  Two events are added, so Desktop must re-pin and its consumer must handle (or
  explicitly drop) the new types. Do not fold it into a restamp-shaped decision.

  observation.rs:329-347 makes a descriptor mismatch a HARD ERROR at `ready`
  (ContractMinorMismatch / SchemaDigestMismatch / FixtureDigestMismatch /
  SourceInputsDigestMismatch / CapabilityStatusMismatch). A Desktop pinned to the old
  descriptor fails to negotiate until it re-pins. That is the coordination cost, stated.

--------------------------------------------------------------------------------
6. WHAT IS DELIBERATELY *NOT* IN THIS REQUEST  (the honest gap)
--------------------------------------------------------------------------------
  NO host->core command. A `goal_resync` pull command must be ANSWERED in the CLI
  command loop, which is crates/wcore-cli/src/main.rs — FENCED to another lane. A
  command variant on the wire with no dispatcher is a capability nothing answers, which
  is the false-advertising class this program has already paid for twice (--skills-promote
  bailing unconditionally, INV-21-23.md:149). So the surface is PRODUCER-PUSH ONLY and
  the capability is registered ShapeOnly, NOT Available, which is the honest status.

  NEXT INCREMENT, for whoever owns main.rs:
    - add ProtocolCommand::GoalResync { goal_version, request_id, goal_id: Option<String> }
    - dispatch it in the main.rs command loop, answering with goal_snapshot per Goal
      (wcore_agent::goal::goal_snapshot_event already builds it; goal_stream already
       builds the ordered replay)
    - flip durable_goals_v1 ShapeOnly -> Available in the SAME change, never before it
```
<!-- ========================== END SEAM REQUEST ========================== -->

## 7. A defect I found in my own instrument, and repaired in-lane (§6b-ii)

My first log grader was `grep -E "^test result:|^error"`. On `wcore-cli --lib` it reported:

```
test result: FAILED. 0 passed; 1 failed; ...
error: test failed, to rerun pass `--lib`
test result: ok. 1844 passed; 0 failed; 1 ignored; ...
```

The first two lines are **not this crate's suite**. `wcore-cli`'s own
`plugin::scaffold::tests::plugin_test_propagates_a_failing_suite`
(`crates/wcore-cli/src/plugin/scaffold.rs:262`) deliberately scaffolds a crate whose single test
panics, runs a **nested `cargo test`** over it, and asserts the failure surfaces as non-zero.
That nested cargo inherits stdout/stderr, so its output splices into the parent's log.

First-match grep therefore gives a **false RED on a green suite**; last-match gives a **false
GREEN** the moment a real failure precedes a nested pass.

**I repaired the instrument rather than writing it up.** `22-C1-EVIDENCE/read-cargo-result.py`.
Its *first* repair attributed each result to the nearest preceding `Running <target>` header —
**and its own self-test failed, correctly**: the nested header lands in the *middle* of the
parent's run, so proximity attributes the parent's result to the child. Any parser claiming
otherwise is guessing. So the instrument **refuses** instead: the outer exit status, captured to
a file rather than read across a pipe, is the verdict; the log only corroborates; and a result
block that executed **zero** tests is graded a failure, not a pass.

Three-assertion self-test, all PASS. The third is the one that proves the repair does something:
first-match grep and this instrument return **opposite verdicts on byte-identical input**.

The second instrument, `contract-drift-probe.py`, carries the same discipline — its third
assertion shows an unframed digest colliding on a path/content byte move where the framed one
separates.

## 8. Honest verdict against criterion 22-C1

| clause | before | after |
|---|---|---|
| CLI surface | MET | MET (unchanged) |
| **host protocol surface** | **absent — 0 symbols** | **PRESENT: 2 typed events, live-proven, 7 emitted** |
| **TUI surface** | **absent — 0 references** | **PRESENT: ingest + state + status-bar segment, 6 tests** |
| serialized producer fixtures Desktop replays at D2 | absent | **defined in `EVENT_SPECS`, NOT yet materialised** — §6 |
| a user *controls* a Goal from TUI/host | absent | **STILL ABSENT** — read-only, §6 ¶6 |

**NOT MET, and closer than it has been.** Two things stand between this and MET, both named:

1. **The fixtures do not exist on disk yet.** They are declared, the generator will emit them,
   and this lane is forbidden to run it. That is §6 and it is one command by the right owner.
2. **The surface is read-only.** "Sees" is delivered on all three surfaces. "Controls" is not —
   it needs a host command answered in `main.rs`, fenced to another lane. I chose an honest
   ShapeOnly capability over a command nothing answers.

I did the protocol first as instructed and the TUI fit too, so I did both.

## 9. What I did NOT do

- Did **not** run `wcore-contract generate` — §6.
- Did **not** touch `.github/workflows/ci.yml`, `crates/wcore-cli/src/{lib,main}.rs`, or
  `.planning/BACKLOG.md`.
- Did **not** add a `ProtocolCommand` variant — §6 ¶6.
- Did **not** put the full task attempt history on the wire (v1 carries a per-task summary +
  `state_digest` over the FULL reduced state; full history stays on `goal status`).
- Did **not** build `tui/surfaces/goals.rs` as a full-screen surface. The TUI shows Goal state
  in the status bar and holds the whole projection in `App.goals`; a dedicated surface is a
  rendering task on state that is now present.
- Did **not** measure on Windows or macOS. Linux only.
- Did **not** wire emission into the live `AgentEngine` event stream — `goal stream` replays the
  chain on demand. A Goal driven by `goal run` does not push events to a connected host in
  real time; that needs the same `main.rs` seam as the pull command.

## 10. Anything the orchestrator must serialize

1. **§6 seam request** — fold into the single contract regeneration over the merged tree.
   Two named events; `CONTRACT_MINOR` bump is vetoable in one constant.
2. **Shared-file fence: nothing.** Diffed against the merge base captured ONCE at lane start
   (`8bcb052b`), never against the branch name — that is the mistake that made lane 24d report
   28 deletions it never made. `git diff 8bcb052b HEAD -- crates/wcore-cli/src/lib.rs
   crates/wcore-cli/src/main.rs .github/workflows/ci.yml .planning/BACKLOG.md` is **empty**,
   and none of those paths appears in the 24-file changed set.

   **That empty result was control-tested**, because an empty diff is exactly the shape a
   silently-broken diff invocation also produces: the identical command against a file I DID
   change (`crates/wcore-protocol/src/lib.rs`) returns **247 bytes of real content**. The
   instrument can produce non-empty output; it produced none for the fenced paths.
3. **`ProtocolEvent` gained two variants.** Any lane with an exhaustive `match` over it will
   need arms. `cargo check --workspace --all-targets` is **0 errors** at `d5a77d02`, so the tree
   is consistent as of this branch — but a lane that branched earlier and adds its own match
   will collide. `tui/protocol_bridge.rs:118` was the only site in-tree.
4. **`.planning/SEAM-REQUESTS/` was not edited** — the request is fenced in §6 of this file so
   the orchestrator can lift it verbatim without a merge conflict against other lanes.
