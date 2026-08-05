# 22-C1 NOTES — durable Goals on the protocol + TUI surfaces

Lane `lane/22-c1`, worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-22-c1`,
base `8bcb052b` (merge-base with `plan/f20-unified-audit-repair`).

## Minute-0 baseline (measured at 8bcb052b, in this worktree)

```
grep -ri 'goal' crates/wcore-protocol/src/ | wc -l   -> 0
grep -ri 'goal' crates/wcore-cli/src/tui/ | wc -l    -> 1   (tui/statusline/mod.rs, unrelated prose)
ls crates/wcore-cli/src/tui/surfaces/goals.rs        -> does not exist
```

Matches `INV-21-23.md:92` exactly. Durable Goals exist on the CLI surface only
(`crates/wcore-cli/src/goal_cmd.rs`, `main.rs:753` `TopCmd::Goal`).

## Objective

1. Protocol surface (the Phase 23 D2 exit gate): typed Goal command + event set in
   `wcore-protocol`, ADDITIVE only. Do not alter `ready` or any existing event shape.
2. TUI surface: a user in the terminal can see goal state.

Scope order per lane instruction: protocol FIRST. If only one lands, it is the protocol.

## Hard constraints carried

- Do NOT run `wcore-contract generate`. Write a fenced seam request into the SUMMARY
  naming exactly which events were added. `observation.rs:329` makes a fixture mismatch a
  hard error at `ready`.
- `golden_v0_1_21.rs` pins the wire contract. Red there == a changed shape.
- Additive events only. A changed event breaks our own Desktop app.
- Do not edit `.github/workflows/ci.yml`, `crates/wcore-cli/src/{lib,main}.rs`,
  `.planning/BACKLOG.md` (other lanes own those).
- No cargo on the Mac except `cargo fmt --all -- --check`. Build/test on `hetzner-dsm`.
- Run test targets by FILE, never by filter. `wcore-agent --lib` must run serially.

## Open questions to establish

- [ ] What is the existing event/command shape convention in `wcore-protocol/src/events.rs`
      and `commands.rs`? (must copy exactly)
- [ ] What does the goal projection that `goal status` emits look like? That is the payload
      the protocol should carry.
- [ ] How does `strategy.rs` (loop-owner kernel, landed overnight) expose transitions? Reuse,
      do not parallel-build.
- [ ] Where does the TUI get its state — `protocol_bridge.rs` vs `engine_bridge.rs`?

## Log

- (t0) worktree created, baseline measured, notes committed.

## Measurement 1 (t+~25m) — the corpus is ALREADY drifted at base, before I touch anything

Instrument: `22-C1-EVIDENCE/contract-drift-probe.py` (self-test 3/3 PASS). It re-implements
`contract/canonical.rs::digest_named_bytes` in Python and reproduces the generator's own
`schema_digest` **exactly** — so the port is proved correct by a value the instrument did not
choose. Under that proof, `source_inputs_digest` does NOT match:

```
schema_digest        computed == recorded   (sha256:e5d1744a…)  -> instrument proved correct
source_inputs_digest computed  = sha256:c9944359…
                     recorded  = sha256:25170996…               -> MISMATCH at base
```

`SOURCE_INPUTS` (spec.rs:~830) hashes the **file bytes** of 40 files including
`crates/wcore-protocol/src/{events,commands}.rs`. Consequences, both load-bearing for this lane:

1. **ANY byte-level edit to `events.rs` or `commands.rs` drifts the contract descriptor.**
   There is no "additive enough to avoid the seam" edit. `observation.rs:342`
   (`SourceInputsDigestMismatch`) makes it a hard error at `ready` for a pinned Desktop.
   So a fenced seam request is unavoidable, exactly as the lane instruction anticipated.
2. **The regeneration is already owed and is not mine.** 3 of 40 SOURCE_INPUTS changed since
   the last authorized re-stamp `5f74d559` (2026-07-28): `wcore-agent/src/bootstrap.rs`,
   `wcore-agent/src/output/protocol_sink.rs`, `wcore-cli/src/main.rs` — all other lanes'
   work. The drift CI job fails at base, which corroborates `INV-21-23.md:95`.

Baseline capture: `22-C1-EVIDENCE/drift-at-base.txt` (970 bytes, rc=3).

### Design consequence

Additive-but-unfenced is impossible. So take the honest additive route: new `ProtocolEvent`
variants + new `HostCommand` variants + new `EVENT_SPECS`/`COMMAND_SPECS`/`PRODUCER_*_TYPES`
entries, all **new wire types**, zero change to any existing variant's shape, and a fenced
seam request naming them. `golden_v0_1_21.rs` must stay green — it pins existing shapes.

Unknown-event safety net measured at `observation.rs:355-369`: an event type NOT in
`PRODUCER_EVENT_TYPES` is dropped when `critical:false`, hard-errors when `critical:true`,
and hard-errors when `critical` is absent.

## Log
- (t0) worktree created, baseline measured, notes committed.
- (t+25m) drift probe written, self-tested 3/3, base drift proved pre-existing.

## Measurement 2 (t+~45m) — the design, and two things I decided NOT to build

### What the wire can reuse rather than re-spell

`wcore-protocol` already depends on `wcore-types`, and `wcore_types::goal` already holds the
canonical taxonomy with `Serialize + Deserialize`: `GoalId`, `GoalStrategy`, `GoalTerminalState`,
`LoopPolicy`, `WaitKind`, `TaskId`. So the wire projection reuses those EXACT types. No second
Goal vocabulary is minted on the protocol side — which is the failure mode 22-02/22-03 kept
naming ("a surface that renders its own shape is a surface that can disagree with the chain",
`goal_cmd.rs:577`).

Only three shapes have no `wcore-types` home and must be mirrored: `GoalLifecycle`,
`GoalAuthorityRecord` and the task ledger — all defined in `wcore-agent/src/session_journal/model.rs`,
which `wcore-protocol` cannot depend on (agent depends on protocol, not the reverse).

**Authority note.** `GoalAuthorityWire` deliberately does NOT reopen the route `record.rs:1-31`
closes. The only function that turns a durable record into an effective envelope is
`GoalAuthorityRecord::reconstruct`, it lives in `wcore-agent`, and it takes
`GoalAuthorityRecord` — not the wire type. A host that deserializes `GoalAuthorityWire` holds a
description, not an envelope.

### DECIDED: no `ProtocolCommand` variant in this lane

I was going to add a read-only `goal_resync` pull command. I am not, and the reason is a
measurement, not caution: a host command has to be answered in the CLI command loop, which is
`crates/wcore-cli/src/main.rs` — **fenced to another lane**. A command variant on the wire with
no dispatcher is a wire type that advertises a capability nothing answers. That is precisely
the false-advertising class this program has already paid for twice
(`--skills-promote` bailing unconditionally, `INV-21-23.md:149`). So: producer events only,
`ContractCapabilityStatus::ShapeOnly` (**not** `Available`) for the new capability, and the
host pull command named explicitly in the seam request as the next increment with its
dispatcher site.

### DECIDED: narrow the task ledger on the wire, and prove the narrowing

`GoalTaskState` carries the full attempt history (`attempts`, `handoffs`, `completion`). Mirroring
it doubles the wire surface for data a control plane summarises anyway. The wire carries a
per-task summary plus a closed `GoalTaskWireStatus`. The honest mitigation is `state_digest`
over the canonical JSON of the FULL `GoalState` — same device `session_recovery_snapshot` uses —
so a host can always tell which chain state its narrowed view corresponds to. Full history stays
on `goal status`. Named in the seam request as a v2 candidate.

### Falsifiable guard against the mirror drifting from the chain

`wcore-agent` gets `goal/wire.rs` with the conversion and a **field-coverage test**: serialize a
fully-populated `GoalState`, collect its top-level JSON keys, and assert every one is either
represented on the wire projection or in an explicit `DELIBERATELY_NOT_ON_THE_WIRE` list. Add a
field to `GoalState` and forget the projection and that test goes red. Without it the mirror
silently rots, which is the whole risk of choosing a mirror over the reduced state itself.

### Contract entries this drifts (all of it goes in the fenced seam request)

- `EVENT_SPECS` 49 -> 51; `PRODUCER_EVENT_TYPES` 57 -> 59
- `SOURCE_INPUTS` gains `crates/wcore-protocol/src/goal.rs` (else the new wire types are
  outside the descriptor's own integrity hash — a real gap, not a formality)
- `contract_capabilities()` gains `durable_goals_v1: ShapeOnly` -> `CapabilityStatusMismatch`
- `CONTRACT_MINOR` 8 -> 9. An additive event set IS a minor bump; leaving it at 8 while the
  event count moves 49 -> 51 would be a dishonest version. Flagged for veto in the seam request.
- `desktop_contract_corpus.rs` count assertion 49 -> 51

## Log
- (t0) worktree created, baseline measured, notes committed.
- (t+25m) drift probe written, self-tested 3/3, base drift proved pre-existing.
- (t+45m) design fixed; command variant and full task ledger explicitly declined with reasons.

## Measurement 3 (t+~2h) — protocol surface lands, first real counts

Built on `hetzner-dsm` at `/root/wayland-22-c1`, branch `hz/22-c1`, HEAD `e0b22b9e`.

```
cargo test -p wcore-protocol --lib
  -> test result: ok. 125 passed; 0 failed; 0 ignored
  -> grep "goal::tests" = 5 lines, all ok   <-- the new module's tests DID execute
```

The grep is not decoration. `--lib` on a crate whose new module failed to compile into the
test binary would still print `test result: ok` for the modules that did; asserting the
executed NAMES is the only thing that proves my five ran. Same class as the `0 of 12` and
`5 passed for zero work` traps in LANE-BRIEF §3.2.

Shipped so far:
- `crates/wcore-protocol/src/goal.rs` — wire projection (new file, in SOURCE_INPUTS)
- `ProtocolEvent::{GoalSnapshot, GoalTransition}` — additive, nothing existing reshaped
- `crates/wcore-agent/src/goal/wire.rs` — conversion + the field-coverage guard + `goal_stream`
- `wayland-core goal stream --journal … --goal … [--expect N]` — the live producer path

## Log
- (t0) worktree created, baseline measured, notes committed.
- (t+25m) drift probe written, self-tested 3/3, base drift proved pre-existing.
- (t+45m) design fixed; command variant and full task ledger explicitly declined with reasons.
- (t+2h) protocol surface committed + pushed; wcore-protocol --lib 125/125 with all 5 new
  goal tests named in the output.

## Measurement 4 (t+~3h) — the corpus test was ALREADY RED at base. Proved with the test itself.

Two independent measurements now agree, and the second is the Rust gate rather than my
Python port of it. Both taken on `hetzner-dsm`.

**At base `8bcb052b`** (`/root/wayland-22-c1-base`, detached, untouched by this lane):

```
cargo test -p wcore-protocol --test desktop_contract_corpus
  -> test result: FAILED. 14 passed; 1 failed; 0 ignored
  -> checked_corpus_matches_real_serializers_byte_for_byte
     drift: drifted=[adversarial/events/{fixture,schema,version}-mismatch.jsonl,
                     events/ready.json, manifest.json]   missing=[]  extra=[]
     "...; run `wcore-contract generate`"
```

**At my HEAD `e0b22b9e`** (`/root/wayland-22-c1`):

```
cargo test -p wcore-protocol --test desktop_contract_corpus
  -> test result: FAILED. 13 passed; 2 failed; 0 ignored
  -> checked_corpus_matches_real_serializers_byte_for_byte
     missing=[events/goal_snapshot.json, events/goal_transition.json]
     drifted=[the same 5, plus schema/{core-event,host-command,producer-complete}.schema.json]
  -> manifest_pins_generator_and_all_three_digests   left: Number(8)  right: 9
     (my deliberate CONTRACT_MINOR 8 -> 9)
```

So: **this gate did not go from green to red in my lane. It went from red to differently-red.**
The regeneration was already owed at base, by three other lanes' edits to SOURCE_INPUTS files
(`wcore-agent/src/bootstrap.rs`, `wcore-agent/src/output/protocol_sink.rs`,
`wcore-cli/src/main.rs`). I am adding two named fixtures and one deliberate minor bump to a
regeneration that has to happen anyway. That is the whole content of the seam request.

I am NOT reporting this as green. It is red, it is red for a reason I can name exactly, and
the fix is a single `wcore-contract generate` over the merged tree by the lane that owns it.

### Gates that stayed green — these are the ones that would have caught a breaking change

```
cargo test -p wcore-protocol --test golden_v0_1_21          -> ok. 22 passed; 0 failed
cargo test -p wcore-protocol --test host_decoder_contract   -> ok. 31 passed; 0 failed
cargo test -p wcore-protocol --test desktop_contract_adversarial -> ok. 17 passed; 0 failed
cargo test -p wcore-protocol --lib                          -> ok. 125 passed; 0 failed
```

`golden_v0_1_21` pins the wire contract. It is green, which is the evidence that the two new
events are genuinely ADDITIVE and no existing shape moved. If I had touched `ready` or any
existing variant, that is where it would have shown.

`inventory_is_exactly_eighteen_commands_and_fifty_one_events ... ok` — the 49 -> 51 count is
real, and that assertion is a gate that goes red on a miscount (it did, at 49, before I
updated it).

## Log
- (t0) worktree created, baseline measured, notes committed.
- (t+25m) drift probe written, self-tested 3/3, base drift proved pre-existing.
- (t+45m) design fixed; command variant and full task ledger explicitly declined with reasons.
- (t+2h) protocol surface committed + pushed; wcore-protocol --lib 125/125, 5 new tests named.
- (t+3h) base-vs-HEAD corpus comparison taken; golden 22/22 green proves additivity.

## Measurement 5 (t+~4h) — LIVE. 7 goal events off the shipped binary, with counts.

Driven on `hetzner-dsm` against `target/debug/wayland-core`, build identity asserted BEFORE
any measurement: `wayland-core 0.12.25 (source 884bca8c1a7bca5f393790bc705af3d1402caa06)` —
my lane HEAD. Caller-generated nonce `22c1-1785289395-14405`. Script and captures committed
under `22C1-EVIDENCE/live/`.

The Goal was opened, given two tasks with a real dependency, and driven through the REAL
`FleetDispatcher` with `--terminate`, so the chain being projected was written by the product:

```
GOAL: run_complete waves=2 iterations=2 completed=2 delivered=2
GOAL: canonical_transition strategy=fleet terminal=Terminated { terminal:
      PartiallyCompleted { completed: 2, failed: 0 } } cursor_seq=Some(17)
```

Then the NEW surface:

```
$ wayland-core goal stream --journal … --goal g-22c1-…
GOAL-STREAM: events=7 transitions=6 snapshots=1
stdout bytes=3164   stderr bytes=77
lines=7  goal_snapshot=1  goal_transition=6  valid_json_lines=7  invalid=0
```

Ordered wire output, decoded:

| # | type | transition | cursor seq | lifecycle after |
|---|---|---|---|---|
| 0 | goal_transition | opened | 0 | opened |
| 1 | goal_transition | run_resumed | 3 | **opened** |
| 2 | goal_transition | loop_owner_claimed | 4 | **opened** |
| 3 | goal_transition | iteration_started | 5 | running |
| 4 | goal_transition | iteration_started | 11 | running |
| 5 | goal_transition | loop_owner_finished | 17 | terminated |
| 6 | goal_snapshot | — | 17 | terminated / partially_completed{completed:2,failed:0} |

**Rows 1 and 2 are the design decision paying off, empirically.** `run_resumed` and
`loop_owner_claimed` both report `opened`, because neither transition determines a lifecycle by
itself. A projection that derived the lifecycle from the transition's NAME would have written
`running` for both and been wrong twice in a seven-event stream. Folding through `replay_state`
— THE reducer — is what makes those two rows right, and it is exactly the case I could not have
guessed.

The snapshot carries the task ledger with dependencies and outcomes intact:

```
task build    completed epoch=1 attempts=1 deps=[]        outcome={"state":"self_checked"}
task publish  completed epoch=1 attempts=1 deps=["build"] outcome={"state":"self_checked"}
iterations 2/4  resume_count 1  loop_owner_epochs 1
```

### Every gate in the drive was falsified in the same run

| gate | falsification | rc |
|---|---|---|
| `--expect 999` (wrong) | must refuse | **1** — "expected 999 goal events, emitted 7" |
| `--expect 7` (right) | must pass | **0** |
| `--goal g-does-not-exist` | must refuse, not print an empty Goal | **1**, stdout **0 bytes** |
| replay determinism | second stream must be byte-identical | sha256 `3b7682b6…` on both |

Every capture byte-counted; `${PIPESTATUS[0]}` avoided entirely — each rc is read from `$?`
immediately after the command, never across a pipe.

## Log
- (t0) worktree created, baseline measured, notes committed.
- (t+25m) drift probe written, self-tested 3/3, base drift proved pre-existing.
- (t+45m) design fixed; command variant and full task ledger explicitly declined with reasons.
- (t+2h) protocol surface committed + pushed; wcore-protocol --lib 125/125, 5 new tests named.
- (t+3h) base-vs-HEAD corpus comparison taken; golden 22/22 green proves additivity.
- (t+3h30) TUI surface: App.goals + bridge arms + status-bar segment + 6 bridge tests.
- (t+4h) LIVE drive: 7 events off the shipped binary, all four gates falsified.
