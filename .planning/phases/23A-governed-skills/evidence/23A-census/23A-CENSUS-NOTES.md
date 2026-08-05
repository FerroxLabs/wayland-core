# 23A-CENSUS — running NOTES (append-only, committed as I go)

Lane `lane/23a-census`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-23a-census`.
Base / lane HEAD at start: `8bcb052b2aa6b1a9e3f2ed00af935a58c92c1f11`
(= `plan/f20-unified-audit-repair` at fetch time).

Per LANE-BRIEF §6b-i this file is committed inside the first 15 minutes and
re-committed after **every** measurement. There is no partial credit for
uncommitted reasoning.

---

## T+0 — established facts (measured, not assumed)

1. **The driver has not moved since before the fix.**
   `git log --format='%h %ad' --date=short -- crates/wcore-eval-scenarios/tests/f23a_boundary_drive.rs`
   → exactly one commit, `481682b0 2026-07-26`.
   The D1/F23A-01-H2 fix is `32a5fc90 2026-07-27`
   ("fix(agent,tools): stop a finished tool call from stranding its turn"),
   with regression tests `81508b74 2026-07-27`
   (`crates/wcore-agent/src/orchestration/d1_refusal_terminal_tests.rs`).
   So the driver predates the fix by one day and **has never been run against
   the fixed engine**. This is exactly the gap this lane exists to close.

2. **`crates/wcore-eval-scenarios/src/governed_skill_drive.rs` DOES NOT EXIST**
   at HEAD (`ls` → No such file). 23A-01's plan called for a shared harness in
   that file and 23A-02/23A-04 both list it in `read_first`. It was never
   written; the driver carries its own private `BoundaryEnv`. Recorded as a
   plan-vs-tree divergence, to be graded in the report.

3. **The driver contains 3 `#[tokio::test]` fns**, not 16 route probes:
   - `refused_skill_tool_call_does_not_kill_the_session` (D1 probe, route R1 shape)
   - `refused_read_tool_call_does_not_kill_the_session` (D1 scope discriminator, NOT a skills route)
   - `generated_draft_is_refused_at_every_route_while_user_content_is_not`
     — this single test is the only one that touches census routes, and it
     drives **R1, R6, R7, R8** and nothing else.
   So on inspection the live driver reaches **4 of 16** census routes. The
   remaining 12 are graded by the census on code reading alone. That is the
   headline hypothesis to confirm or refute by measurement.

4. **The census's own SHA is `2ecdfdf5`, not HEAD.** Every `path:line`
   citation in `23A-01-SURFACE-CENSUS.md` must be re-resolved at
   `8bcb052b` before it can be called current; line numbers will have drifted
   and at least `orchestration/mod.rs` and `skill_tool.rs` were edited by the
   D1 fix itself.

## T+0 — what I still have to establish

- [ ] Re-resolve all 16 route citations at HEAD (Mac-side read; no cargo).
- [ ] Build `wayland-core` at HEAD on hetzner and run the driver, reading back
      the executed count (`N passed`) per target — NEVER the exit status, and
      never via a `-- <filter>` (flavour (c) of the zero-tests trap).
- [ ] Two-run differential for `WAYLAND_F23A_SELFTEST=refusal`: run A without
      the env var, run B with it. **They must DISAGREE.** Two runs that agree
      prove nothing about the control.
- [ ] Grade each of the 16 routes: live-driven / static-only / undrivable, with
      the reason.

---

## T+35 — MEASURED: the driver runs at HEAD, and the selftest FIRES

Hetzner worktree `hz/23a-census` at `/root/wayland-23a-census`, commit
`dedd13d7` (= lane HEAD, tree identical to `8bcb052b`). Build
`cargo build --locked -p wcore-cli --bin wayland-core` → `WLRC=0`.
`WCORE_EVAL_BIN=/root/wayland-23a-census/target/debug/wayland-core`.

Target run **by file**, never by filter:
`cargo test --locked -p wcore-eval-scenarios --features packaged-driver-gate
--test f23a_boundary_drive -- --nocapture --test-threads=1`

### Two-run differential — they DISAGREE

| Run | `WAYLAND_F23A_SELFTEST` | executed count read back | sentinel |
|-----|------------------------|--------------------------|----------|
| A | unset | `3 passed; 0 failed; 0 ignored; 0 filtered out` | `WLRC=0` |
| B | `refusal` | `2 passed; 1 failed; 0 ignored; 0 filtered out` | `WLRC=101` |

Run B printed `F23A-SELFTEST-TRIPPED: refusal` and panicked with
`/skill run refusal did not fire for the substituted control`.

**The control has teeth.** This is the first time it has been shown to fire.
Captures: `run-A-baseline.log` (6022 bytes, byte-count matched against the
remote), `run-B-selftest-refusal.log` (1061 bytes),
`run-status-sentinels.txt` (201 bytes). Counts read back from `N passed`, never
from the exit status.

**Also measured, and it is the substantive result of run A:** the two D1 probes
`refused_skill_tool_call_does_not_kill_the_session` and
`refused_read_tool_call_does_not_kill_the_session` — committed RED at
`481682b0` as the F23A-01-H2 reproducer — both pass at HEAD. The fix
`32a5fc90` holds under the live driver.

### INSTRUMENT DEFECT found in the same run (§6b-ii applies)

The selftest short-circuits. `trip_selftest` is `-> !`; it panics on the FIRST
route check that fails to refuse. Run B tripped at `/skill run` and **never
evaluated the other three route checks at all**. So the differential proves the
R6 assertion has teeth and says NOTHING about the R7 / R8 / R1 assertions in
the same test.

Predicted (to be measured after the repair) — under substitution
`probed_name = "f23a-control"`:

- R6 `/skill run` → trips (measured).
- **R7 `/skill list` → will NOT trip, because the matcher is vacuous.** It is
  `info.contains(&probed_name) && info.contains("(hidden)")` — two unbound
  substring searches over the joined info events. The real `auto-*` draft is
  still in the catalog and still tagged `(hidden)`, so the second conjunct is
  true no matter what the probed name is. The correct matcher must bind the tag
  to the name on one line: `runtime_list` renders
  `"  - {name}{tag} [src={src}]"` (`slash/skill.rs:148-160`), so the binding
  form is `- {name}(hidden)`.
- R8 `/skill show` → should trip (control renders `visible to model`).
- R1 `Skill` tool → should trip (control resolves, `is_error` false).

Repairing in-lane per §6b-ii: a written-up instrument defect is a defect I have
agreed to keep, and the recorded precedent is that the next lane hits it again.

---

## T+75 — instrument repaired, and the vacuity is now MEASURED not argued

Repair commits `159682e9` (short-circuit + binding matcher + 3-assertion
self-test) and `7b5ee047` (live legacy-matcher diagnostic). Hetzner worktree
reset to each in turn; four more runs.

| Run | commit | selftest | executed count read back | sentinel |
|-----|--------|----------|--------------------------|----------|
| C | `159682e9` | unset | `4 passed; 0 failed; 0 ignored; 0 filtered out` | `WLRC=0` |
| D | `159682e9` | `refusal` | `3 passed; 1 failed; 0 ignored; 0 filtered out` | `WLRC=101` |
| E | `7b5ee047` | unset | `4 passed; 0 failed; 0 ignored; 0 filtered out` | `WLRC=0` |
| F | `7b5ee047` | `refusal` | `3 passed; 1 failed; 0 ignored; 0 filtered out` | `WLRC=101` |

Count went 3 → 4 because the repair adds `list_tags_hidden_matcher_selftest`.
The baseline stays green under a STRICTLY STRONGER R7 matcher — nothing was
weakened to reach it.

Run F, the load-bearing lines, verbatim:

```
F23A-SELFTEST-ROUTE: R6 /skill run    refused=false
F23A-SELFTEST-ROUTE: R7 /skill list   refused=false
F23A-SELFTEST-ROUTE: R8 /skill show   refused=false
F23A-SELFTEST-ROUTE: R1 Skill tool    refused=false
F23A-SELFTEST-LEGACY: R7 /skill list  old_matcher=true
F23A-SELFTEST-TRIPPED: refusal
... not discriminating: []
```

**All four route checks now discriminate** (`toothless == []`), where before the
differential exercised exactly one of them.

**And the R7 vacuity is measured on the live bytes:** the pre-repair matcher
reports `true` — "the quarantined draft is tagged hidden" — while being handed
the user-authored, model-VISIBLE control. It was a self-passing gate of the
kind LANE-BRIEF §3.2 enumerates, sitting inside the instrument built to hunt
that class. (Corrected on review: I wrote "twelfth recorded instance" here and
cannot substantiate the ordinal — the brief says eleventh, the dispatch says
twenty-plus. The report says "another instance" instead. An unverifiable count
is exactly the kind of detail that should not be asserted.)

Captures: `run-C..F-*.log`, byte counts 633 / 1628 / 633 / 1752, each matched
against the remote `wc -c` before and after transfer.

## Traps I am holding (from the brief)

- Byte-count every capture; `${PIPESTATUS[0]}` after a pipeline returns empty here.
- `wcore-agent --lib` fails 13–19 in parallel, passes 2160/0 serial → run serially.
- Instruments carry the defect they hunt; repair in-lane with a 3-assertion
  self-test whose third assertion is "the old shape would have missed it".
- Run targets by file (`--test <name>`), never by filter.
