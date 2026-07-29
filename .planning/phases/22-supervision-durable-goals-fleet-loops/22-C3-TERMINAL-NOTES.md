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
- [t1] Cross-audit panel (codex 5.6-sol / gemini 3.1-pro / kimi K3 + internal adversarial) on the
  design. **Unanimous on two points:** (a) making `resolve()` total is NOT structural closure by
  itself — a future caller can simply not call it; (b) `engine.run` is the wrong enforcement
  boundary for Direct (it is a per-TURN loop; a Goal spans many turns and many engines).
  Split on the rest: codex/gemini wanted capability receivers + private raw drivers across all
  five (measured cost: ForgeFlows ~34 call sites, Direct thousands — not achievable in one lane);
  kimi argued the conversion boundary is already sealed and the real fix is to delete the
  non-canonical *termination* paths, at zero call-site churn. **Internal adversarial pass sided
  with kimi**, and measurement then settled it.
- [t2] **THE FINDING.** The bypass is not (only) in the CLI's `if let Some(...) = resolve()`.
  It is in the reducer. `GoalKernel::terminate` is public and `SessionEvent::GoalTerminated`
  was refused ONLY while a loop owner was live:

      if let Some(owner) = &goal.loop_owner { refuse }

  with the stated justification that *"A Goal that never claimed an owner is still terminable
  this way — but by definition no engine ran it, so there was no loop owner to be canonical
  about."* **That premise is false precisely when attachment is opt-in.** An engine can run a
  Goal to completion, never claim, and then record `SelfChecked` — a full engine verdict — down
  the plain path. A sixth engine added tomorrow is in that state BY DEFAULT, since claiming is
  the thing it would have to opt into. This is the brief's "sixth engine cannot bypass it"
  scenario, and it was representable.
- [t3] Built the closure: exhaustive `GoalTerminalState::requires_loop_owner()` split
  (engine-produced vs control-plane, no wildcard arm) + unconditional reducer refusal of an
  engine verdict on the plain path. Workspace check green.
- [t4] The closure made **5 existing tests fail** — all of them terminating a Goal with an
  engine verdict on the raw path. Adapted each to the sanctioned route; no assertion relaxed,
  nothing `#[ignore]`d. Kernel round-trip keeps its serialization property in full (engine half
  moved to a crate-internal test over the canonical transition; integration test gained the
  refusal assertion over every engine shape — coverage is a superset of the original).
- [t5] wcore-agent: **3205 run, 3205 passed, 0 failed** (3 flaky, retried green).
- [t6] Per-owner three-assertion gate written (`goal_no_bypass_test.rs`), all five refusals
  captured verbatim. **Falsified:** neutralizing the new refusal (restoring the old shape) turns
  5/5 per-engine cases + the sweep RED — and the failure output shows the old shape returned a
  successful `RecoveryCursor { journal_sequence: Some(1) }`, i.e. the bypass write COMMITTED.
  Reducer restored via `git checkout -- <one path>` (permitted; moves no ref), verified clean.
