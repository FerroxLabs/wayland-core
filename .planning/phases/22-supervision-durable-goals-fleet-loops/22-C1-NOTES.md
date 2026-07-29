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
