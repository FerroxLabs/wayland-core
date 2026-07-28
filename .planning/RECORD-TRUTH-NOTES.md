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

### T1.d Still to establish
- the live evidence (control child 8 turns vs narrowed child 3) — locate + verify
- whether the canary tests (`f21_02_no_channel_canary.rs`) now assert PRESENCE
- run the relevant tests on hetzner (targeted, `-p` only)

---

## T2 — F23A-01-H2 — not yet started
## T3 — dropped-findings sweep — not yet started
## T4 — CRITERIA-GAP-LEDGER rows — not yet started
