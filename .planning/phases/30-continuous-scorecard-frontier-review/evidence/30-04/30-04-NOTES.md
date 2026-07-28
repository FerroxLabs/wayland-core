# 30-04 NOTES — running log (committed early per LANE-BRIEF §6b-i)

Lane: `lane/30-04`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-30-04`.
Base: `fced9f6189ab66c74a2cb452f9ecab4da094893e` (= `plan/f20-unified-audit-repair` after 30-03 merge).

## t+0 — established

- Worktree created, `git rev-parse --show-toplevel` verified as the lane path (NOT `/Users/seandonahoe/dev/waylandcore`).
- `plan/f20-unified-audit-repair` local == `gh/plan/f20-unified-audit-repair` == `fced9f61`.
- 30-04-PLAN.md read in full (366 lines). Three tasks:
  1. `reserved_authority` module + contract suite (TDD).
  2. Both authority runs on hardware + no-reserved-action audit with stated ceiling.
  3. Grade the four Success Criteria verbatim, positioning packet, residuals.
- Termination states: exactly three. Complete / Complete-with-criteria-NOT-MET / Escalated.
  Expectation stated in the plan itself: state 2 is the likely one.

## t+35 — inputs read, grading inputs established

All four inherited artifacts read. Key facts pinned:

### Criterion 1 evidence (30-01)
- 148 surfaces walked off the shipped `wayland-core` binary (sha256 `e73453a5…`), regeneration
  diff IDENTICAL vs committed bytes.
- `operator_completeness` **UNPROVEN on 148/148**. `peer_delta` **UNPROVEN on 148/148**
  (measured, not assumed — 30-03 §9 confirms unchanged since 30-01 commit `4f749251`).
- maturity / security_authority_owner / evidence **UNPROVEN on 15 rows** (6 top-level
  commands owned by no CTRL-01 family: `init mcp-serve models profile project-context setup`).
- Alias blind spot: `forgeflows` is a hidden clap alias, runs live, has NO inventory row.
  So the inventory is the binary's *advertised* tree, not its *accepted* surface.
- Linux only. macOS/Windows command trees unmeasured.
- "refreshed at each phase": this is a FIRST refresh, not a demonstrated cadence.
- 30-01 filed 6 HIGH (PEER-PROBE-2026-07-26 unresolvable + STALE-01/02/11/12/13).

### Criterion 2 evidence (30-02)
- 15 legs: **9 RUN, 6 UNPROVEN**. security ×3 (meter records digests not bodies),
  cognitive_tax ×3 (panel unanimous: not measurable in this tier).
- So **2 of 5 dimensions have ZERO legs for all three tools.**
- The 9 RUN legs are CONFOUNDED (script emits `write_file`, a Hermes-only tool name;
  OpenClaw also 0/30). LIM-20.
- Pinned baselines DO re-verify (Hermes 0.17.0 @ dbe734be, OpenClaw 2026.6.2 @ 11a0ad10).
- Confidence bounds DO exist on the 9 RUN legs (Wilson / ZERO_EMPIRICAL_VARIANCE).

### Criterion 3 evidence (30-03)
- 9 allowed claims, **0 comparatives**, 20 limitations, 10 refusals, 12 rules, 24 corpus rows,
  12 distinct rules fired. Re-render byte-identical + tamper DETECTED + publish REFUSED on a
  broken reference. Suite 485/485.
- Residual stated by 30-03 itself: the checker cannot prove a sentence MEANS what the evidence
  shows. Lexicon is finite.

### Criterion 4 — 28/29 verdicts EXIST (the plan anticipated they might not)
- `28-04-PHASE-VERDICT.md` EXISTS: C1/C2/C3 **MET WITH STATED EXCEPTIONS**, C4 **NOT MET**.
  Acceptance gate did NOT pass — 2 of 3 receipt claims false. A3 enforced.
- `29-PHASE-VERDICT.md` EXISTS: **goal NOT achieved, all four criteria PARTIAL.**
- **ROADMAP.md's status column is STALE**: its Phase 28 row says "no phase verdict exists yet /
  28-04 not started" and its Phase 29 row says "29-03 and 29-04 not started" — both false
  against the tree. This is 30-01's STALE-06/07/08 still unrepaired. Finding to file.

### LIM-18 RE-CHECKED (do not inherit 30-03's snapshot)
- `.planning/HEADLESS-KEYRING-FINDING.md` merged at `769d98b3`; RC item 7 closed at `2a306ac8`.
- The advertised remedy was dead in **three** independent ways; measured over 11 live routes;
  fixed (`eabb6ec0`) and re-proven. **LIM-18's substitution point is now DISCHARGED** —
  its "result not in" evidence tag is superseded. Record, do not edit 30-03's published doc.

## Still to establish

- [ ] F-30-03-001 (LOW) fix at source in `30-02-SUMMARY.md` Gates prose
- [ ] Task 1: `reserved_authority.rs` + contract suite (TDD, RED recorded)
- [ ] Task 2: both authority runs on hetzner + no-reserved-action audit
- [ ] Task 3: verdict, positioning packet, residuals
