# 22-C3-TERMINAL — working notes (append-only, committed early per LANE-BRIEF §6b-ii)

Lane `lane/22-c3-terminal`. Base asserted `5be910561f688c75d39492e7b982d6e100772a64`
(matched against `/usr/bin/git ls-remote gh plan/f20-unified-audit-repair`).

## 1. My brief's three measurements — re-verified at base

Per LANE-BRIEF "Your brief's MEASUREMENTS are probably stale". All three re-run with
`/usr/bin/grep` (unproxied), each absence carrying a known-positive in the same sweep.

| # | Brief claim | Verdict at my base | Evidence |
|---|---|---|---|
| 1 | `GoalTerminalState` has consumers **only** in `goal/{ledger,kernel}.rs` and `session_journal/model.rs` | **FALSE** | 24 files across 4 crates: `wcore-types`, `wcore-protocol` (3), `wcore-agent` src (8) + tests (7) + examples (1), `wcore-cli` (3) |
| 2 | Grep for `GoalTerminalState` under `orchestration/` returns zero hits | **TRUE but the instrument is defective** | 0 hits confirmed; known-positive `ClimbOutcome` in the same dir → 21 hits, so the grep is alive. The adapter was never built under `orchestration/` — it lives in `goal/strategy.rs`. The ledger's own CORRECTION (c) already identified this as a self-*failing* gate. |
| 3 | Anvil still returns `ClimbOutcome` (`anvil/engine.rs:246,346`) | **TRUE in substance, stale in line numbers** | `ClimbOutcome` struct at `engine.rs:247`; returned at `:362` and `:614`. Lines 246/346 have drifted. |

## 2. The finding that reframes the lane

`crates/wcore-agent/src/goal/strategy.rs` is **present in integration at my base**
(41,803 bytes). So is `22-C3-SUMMARY.md` (19.9 KB) and `22-C3-NOTES.md` (52 KB).

This falsifies **both** the ledger row AND the ledger's own correction:

- The row says FAILED / "no lane has attempted it" — false.
- The correction (2026-07-29, `lane/record-truth`) says the work "is NOT in the
  integration branch" at `ef1d97be`. Integration has since moved to `5be91056` and the
  work **is** present. Correction (b) is now itself stale.

**Two lanes have already worked C3.** Lane 1 (`lane/22-c3`) built the adapter surface.
Lane 2 (`lane/22-c3-goal`) wired all five production paths and live-proved them on the
shipped 0.12.25 release binary, self-grading PARTIAL.

## 3. What actually remains — prior lane's §9.1

The prior lane's own honest ceiling:

> **Attachment is opt-in.** An engine invoked with no Goal still runs and terminates its
> own way. That is **convention, not construction** [...] Making an engine structurally
> incapable of terminating outside a Goal still requires threading a token through five
> entry points.

That is precisely and only what my brief asks for:

> Make the canonical transition the **only** way to terminate, so a sixth engine added
> tomorrow cannot bypass it.

So my lane is not "build the adapter surface" (built), nor "wire the five owners"
(wired, live-proven). It is the **structural-impossibility half** alone.

## 4. Plan

1. Measure the true bypass surface: for each of the five owners, what can a caller do
   today that terminates the engine without touching a Goal.
2. Decide construction vs. failing-test per engine, and say which was achieved.
3. Grade the two halves (terminal-state / no-nested-owner) separately.

## 5. Progress log

- [t0] Worktree created, SHA asserted, brief + ledger + prior summary read.
- [t0] Three brief measurements re-run: 1 FALSE, 1 TRUE-but-defective-instrument, 1 TRUE-with-stale-line-numbers.
