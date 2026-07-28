# RECORD-TRUTH lane — running NOTES (append-only, re-committed after each measurement)

Lane branch `lane/record-truth`, worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-record-truth`.
BASE (merge-base, captured once) = `ef1d97beb61f1b084bdfba745e8f49830924d757`.

All git via `/usr/bin/git` (never `rtk` — it filters `git log`).

---

## T1 — Phase 21 / F21-02 re-grade. Measurements so far

### T1.a Commit existence + timeline — CONFIRMED

| SHA | date (iso, +0700) | subject |
|---|---|---|
| `ac94b1d5` | 2026-07-27 07:58:42 | tools: make worktree rescue produce patches that actually apply |
| `10947402` | 2026-07-27 08:47:45 | feat(21-02): sub-allocate a narrowed execution envelope to delegated children |
| `373599ea` | 2026-07-27 08:54:18 | test(21-02): invert the no-channel canary to assert the channel exists |
| `d12d7d48` | 2026-07-27 09:17:17 | test(21): unblind the budget no-channel canary |
| `d29413c1` | 2026-07-27 21:07:51 | fix(corpus): grade the budget legs on enforcement, not on channel absence |

`21-REVERIFICATION.md` frontmatter says `verified_at_sha: ac94b1d5...`. The four
build commits all land AFTER it, same day, 08:47 → 21:07. **The grading-precedes-
the-work claim in the brief is CONFIRMED on timestamps.**

Caveat to carry: the doc's frontmatter `verified: 2026-07-27T01:30:00Z` == 08:30
+0700, which is 32 min AFTER `ac94b1d5`'s commit time and 17 min BEFORE `10947402`.
Either way the grade predates the channel landing.

### T1.b Symbols at HEAD — CONFIRMED PRESENT

- `pub struct ChildBudgetRequest` — `crates/wcore-types/src/spawner.rs:555`
- `pub budget: Option<ChildBudgetRequest>` — `crates/wcore-types/src/spawner.rs:597`
- `pub fn sub_budget_narrowed` — `crates/wcore-budget/src/execution.rs:586`
- production caller — `crates/wcore-agent/src/spawner.rs:1350`
  (second production caller at `spawner.rs:1377`)
- first `#[cfg(test)]` in `spawner.rs` is line **1448** ⇒ 1350 and 1377 are both
  genuinely production, not test.
- request is parsed from the delegate tool input: `crates/wcore-tools/src/delegate.rs:105`
  `fn parse_budget(input: &Value) -> Option<ChildBudgetRequest>` ⇒ the channel is
  **child/caller-fillable from a shipped tool surface**, which is the exact thing
  §4 said did not exist.

### T1.c §4's three measurements, re-measured at HEAD — SPLIT RESULT

§4 gave three independent measurements. They do **not** all fall. Precise result:

1. **STILL TRUE at HEAD.** `begin_active_turn(turn_id, None)` — sole production
   caller, now at `engine.rs:6203` (doc cited 6173). Other hits are all inside
   `budget_authority.rs` tests. So measurement 1 survives — **but it is about the
   per-turn engine path, not the child-spawn path**, so it no longer supports the
   F21-02 conclusion it was used for.
2. **FALSE at HEAD.** Doc: *"the only `sub_budget(Some(..))` call site in the crate
   sits inside `#[cfg(test)] mod tests`. Zero production callers pass `Some(..)`."*
   At HEAD `crates/wcore-budget/src/execution.rs:591` is
   `self.sub_budget(Some(narrowed))`, inside the body of `sub_budget_narrowed`.
   First `#[cfg(test)]` in that file is line **964** ⇒ line 591 is production.
3. **FALSE at HEAD.** Doc: *"no `crates/*/src` file forwards a `Some(..)` override
   into `sub_budget`."* `spawner.rs:1350`/`1377` forward a caller-supplied
   `ChildBudgetRequest` into `sub_budget_narrowed`, which forwards `Some(..)` on.

Semantics check (guards against "channel exists but cannot bind"):
`sub_budget_narrowed` intersects the request with `self.effective_budget()`
before passing it down, so an adversarial LARGER request cannot amplify — it can
only under-allocate the caller's own descendant. That is the safe direction.

**Working conclusion (to be firmed with the live evidence + test run):** F21-02 is
carried NOT MET on a §4 premise that is 1/3 intact and 2/3 false at HEAD, and the
intact third is about a different code path than the requirement.

### T1.d Live evidence — LOCATED and it is strong
`.planning/phases/21-child-authority-and-budget-inheritance/21-02-VACUITY-SUMMARY.md`
(lane `lane/f21-02-vacuity`, evidence host hetzner-dsm). §3.1 records, from
`crates/wcore-cli/tests/f21_02_child_budget_live.rs`, two runs of real
`wayland-core acp serve` differing ONLY by the `budget` object the parent's own
model puts on its `Delegate` call:

```
F21-02 LIVE: control child served 8 turns, narrowed child served 3 turns
under a 900-token sub-allocation of a 100000-token root.
test result: ok. 10 passed; 0 failed; finished in 4.71s
```

§3.2 records the mutation control: revert the spawn seam to unconditional
`sub_budget(None)` and the narrowed child is served **8** turns — differential
collapses. So the gate is proven red-able, not merely green.

### T1.e The canary is genuinely INVERTED — verified by reading it
`crates/wcore-agent/tests/f21_02_no_channel_canary.rs` header states it inverts
the old absence-canary and asserts the channel EXISTS. It reads only
`crates/*/src`, and asserts its own crawl collected >100 files so a broken walk
cannot make it vacuous. This is the correct repair of a self-passing gate.

### T1.f The 21-02 lane's own F-1 (HIGH) was subsequently FIXED
21-02-VACUITY-SUMMARY §5 F-1 said the phase corpus's `budget_no_channel_canary`
greps the literal `sub_budget(Some(` while excluding `crates/wcore-budget/`, so it
still reported "NO-CHANNEL canary intact" against a live channel — a blind gate.
At HEAD `budget_no_channel_canary` returns **0 hits** in
`crates/wcore-cli/tests/child_authority_corpus/surfaces.rs`; commit `d29413c1`
("grade the budget legs on enforcement, not on channel absence", +218/-42 across
the corpus) repaired it. F-1 is CLOSED at HEAD.

### T1.g F-2 (MEDIUM) was explicitly routed to BACKLOG and NEVER ARRIVED
21-02-VACUITY-SUMMARY §5 F-2: `max_iterations` unclamped
(`delegate.rs` → `spawner.rs:2283`), stated "per the brief's severity policy this
is BACKLOG, not blocking". `grep -c max_iterations .planning/BACKLOG.md` = **0**.
Third independent sighting of the drop pattern → feeds T3.

---

## T2 — F23A-01-H2

### T2.a The fix is REAL at HEAD
- `81508b74` 2026-07-27 08:15:18 — *test(agent): prove D1 — a completed tool error
  strands the turn*. Adds `crates/wcore-agent/src/orchestration/d1_refusal_terminal_tests.rs`
  (RED first — correct TDD order).
- `32a5fc90` 2026-07-27 08:27:20 — *fix(agent,tools): stop a finished tool call from
  stranding its turn*. 7 files, +240/-36, incl. `orchestration/mod.rs` (+135) which
  is exactly the `PreparedToolLease::start` → `lease.fail(...)` span the seam
  request named as the suspected leak.
- Module IS wired: `orchestration/mod.rs:78` `mod d1_refusal_terminal_tests;` —
  so the five tests actually compile and run, not orphaned.
- Five tests, matching the seam request's three triggers plus two more:
  `refused_read_leaves_turn_committable`, `failed_grep_...`, `failed_glob_...`,
  `opaque_shell_error_...`, `approval_denial_control_...` (the last is a control).

### T2.b The census underneath is genuinely STALE — brief confirmed
`crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs` last touched
`481682b0` **2026-07-26 22:49:40**, i.e. the day BEFORE the 07-27 fix. So the
16-route quarantine census has NOT been re-measured at HEAD.
`WAYLAND_F23A_SELFTEST` exists (`f23a_boundary_drive.rs:21,41`) — control is
present in source; whether it was ever made to FIRE is the open question.

## T3 — dropped-findings sweep — IN PROGRESS
Confirmed dropped so far (grep count in `.planning/BACKLOG.md` = 0 for each):
F23A-01-M1, F23A-01-M2, F23A-01-M3 (MEDIUM), F23A-01-H2 (HIGH), plus 21-02 F-2.
Whole-phase gaps in BACKLOG: **Phase 22 = 0, 23A = 0, 23B = 0, 27 = 0** entries.

## T4 — CRITERIA-GAP-LEDGER rows — not yet started
