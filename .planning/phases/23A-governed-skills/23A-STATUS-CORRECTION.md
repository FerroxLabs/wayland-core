---
phase: 23A-governed-skills
correction_date: 2026-07-29
corrected_by: lane/record-truth
base_sha: ef1d97beb61f1b084bdfba745e8f49830924d757
scope: "record correction only — no product change, no build, no test execution"
---

# Phase 23A — status correction (2026-07-29)

Every Phase 23A document records `F23A-01-H2` as open and red. **It was fixed on
2026-07-27.** The bodies of those documents are deliberately left as written —
they were correct when written, and rewriting a prior lane's conclusions from a
later lane is a hazard this program has already had to correct once. This file
is the correction; the documents point at it.

---

## 1. `F23A-01-H2` — FIXED at `32a5fc90`, verified at HEAD

**Original finding** (`.planning/SEAM-REQUESTS/23A.md` SR-23A-2, HIGH): any tool
call returning an error result killed the session. The errored tool execution was
left nonterminal in the session journal and the engine aborted with
`Session persistence authority unavailable: invalid journal state transition:
turn <id> has nonterminal tool execution <id>`. Reproduced live on Linux at
`2ecdfdf5` in 13 s with three independent triggers — a `Skill` call for an absent
name, a `Skill` call for a quarantined generated draft, and a `Read` of a
nonexistent path, the last proving it was not skills-specific.

**Verified at HEAD by lane/record-truth:**

| SHA | committed (+0700) | what |
|---|---|---|
| `81508b74` | 2026-07-27 **08:15:18** | `test(agent): prove D1 — a completed tool error strands the turn`. Adds `crates/wcore-agent/src/orchestration/d1_refusal_terminal_tests.rs`. **RED first** — correct order. |
| `32a5fc90` | 2026-07-27 **08:27:20** | `fix(agent,tools): stop a finished tool call from stranding its turn`. 7 files, **+240 / −36**. |

The fix touches `crates/wcore-agent/src/orchestration/mod.rs` (**+135**), which is
exactly the `PreparedToolLease::start` → `lease.fail(...)` span the seam request
named as the suspected leak, plus `engine.rs`, `recovery_confidential.rs`,
`wcore-config/src/credentials.rs`, and the `glob`/`grep`/`read` tools.

**The five regression tests are wired and will actually run** —
`orchestration/mod.rs:78` declares `mod d1_refusal_terminal_tests;`, so they are
not an orphaned file:

- `refused_read_leaves_turn_committable` — the seam request's `Read` trigger
- `failed_grep_leaves_turn_committable`
- `failed_glob_leaves_turn_committable`
- `opaque_shell_error_leaves_turn_committable`
- `approval_denial_control_leaves_turn_committable` — **a control**, which is what
  makes the other four falsifiable rather than a blanket assertion

**Not re-run by this lane.** This is a source-and-git verification on the Mac. The
tests exist, are wired, and the fix is in the right place; I did not execute them.

## 2. The gap underneath it — the 16-route census is UNMEASURED at HEAD

**Do not read "H2 is fixed" as "the census is done". They are different
measurements, and only one of them has been taken.**

`crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs` — 23A's own reproducer
and the driver of the 16-route quarantine census — was last changed at
`481682b0`, **2026-07-26 22:49:40**, the day *before* the fix. The product it
drives changed on 2026-07-27. So every census number on record was produced
against a build whose dominant failure mode has since been removed.

**The phase's own control was never shown to fire.** `WAYLAND_F23A_SELFTEST`
exists in source (`f23a_boundary_drive.rs:21` and `:41`,
`std::env::var("WAYLAND_F23A_SELFTEST").as_deref() == Ok("refusal")`) and
substitutes a user-authored control skill. No evidence exists in any Phase 23A
artifact that it was ever set and observed to change the outcome. An unfired
control is an unvalidated instrument — the class this program has now recorded
twelve times.

### What re-running it needs (I could not do it; here is precisely why and what)

The lane brief forbids running cargo on the Mac, and this lane has no hetzner
worktree. Concretely, closing this needs:

1. A hetzner worktree at a SHA containing `32a5fc90`, `export PATH=/root/.cargo/bin:$PATH`.
2. `cargo test -p wcore-eval-scenarios --test f23a_boundary_drive` — and **read
   back the executed count**, not the exit status. A filter or an all-`#[ignore]`
   suite exits 0 having run zero tests; assert `N passed` where N is the number of
   routes you expect, and state N.
3. The control, run twice, as a differential: once with `WAYLAND_F23A_SELFTEST=refusal`
   and once without, asserting the two runs **disagree**. If they agree, the
   control is inert and the census proves nothing regardless of its numbers.
4. Re-state the 16 routes with their verdicts at that SHA, and say explicitly
   which changed relative to the pre-fix census.

Tracked as `F23A-01-CENSUS-UNMEASURED` (MEDIUM) in `.planning/BACKLOG.md`.

## 3. `F23A-01-M1`, `M2`, `M3` and `H2` were never filed anywhere

All four were routed to `.planning/BACKLOG.md` via `.planning/SEAM-REQUESTS/23A.md`
for a later lane to paste in. **Grepping `BACKLOG.md` for any of the four returned
zero.** Nobody consumed the file. They are now filed, at the severities their
finder gave them, in the `From lane/record-truth` section of `.planning/BACKLOG.md`.

`F23A-01-H1` is correctly absent — it was **fixed**, not dropped, and is recorded
as such so nobody re-files it.

## 4. What this correction does NOT change

- **`23A-C1` remains NOT MET.** Governed promotion, revocation and rollback are
  still unimplemented. The `--skills-promote` flag is now hidden (see
  `.planning/CRITERIA-GAP-LEDGER.md`, corrected the same day), which closes the
  *advertisement* complaint only.
- **23A-02 was not executed**, and this changes nothing about that.
- No Phase 23A success criterion moves on this correction.

_Corrected 2026-07-29 · base `ef1d97be` · lane/record-truth · source measurement only, no build_
