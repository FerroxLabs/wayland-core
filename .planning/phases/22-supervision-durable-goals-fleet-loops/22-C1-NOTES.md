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
