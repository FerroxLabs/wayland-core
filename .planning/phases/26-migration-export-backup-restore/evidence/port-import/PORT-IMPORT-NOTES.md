# PORT-IMPORT lane — running notes

Lane `port-import`, worktree `lane-port-import`, base integration `b2ddf113`.
Appended after every measurement (LANE-BRIEF §6b-i).

## T0 — the brief's central premise is FALSE at HEAD

The brief and the `PORT-*` ledger row both assert:

> Criterion 2 ... and Criterion 4 ... are both entirely unmet: **plans 26-02 and
> 26-04 were never started**, and the F26-02 quarantine contract ... is **still
> unbuilt**. Nothing has yet imported anything.

Measured at base `b2ddf113`, all four clauses are false:

| Brief claim | Measured | Verdict |
|---|---|---|
| 26-02 never started | `26-02-PLAN.md` + `26-02-SUMMARY.md` (status: complete, termination_state 1) | **FALSE** |
| 26-04 never started | `26-04-PLAN.md` + `26-04-SUMMARY.md` + `26-04-CERTIFICATION.md` (termination_state 2) | **FALSE** |
| quarantine contract unbuilt | `crates/wcore-cli/src/migrate/quarantine.rs`, 1132 lines | **FALSE** |
| nothing has imported anything | `apply_plan()` at `migrate/mod.rs:871`, `ApplyGuard::open` at :826 | **FALSE** |

Git provenance (`/usr/bin/git log`, unproxied):

```
ec9794b1 2026-07-28 feat(26-02): classify and contain imported executable content under the GHSA-8r7g contract
72bc9099 2026-07-28 fix(26-02): compile fixes — loader takes &Path, drop dead helper, update export call sites
aa38ca5f 2026-07-28 style(26-02): satisfy clippy collapsible_if and redundant to_string
76befc9f 2026-07-28 fix(26-02): promoting a set that reuses one skill name costs ONE invocation
a170ee24 2026-07-28 fix(migrate): write the quarantine index atomically (F26-GAPS-H1)
1c13e9a2 2026-07-28 test(26-04): hostile suite and the mirrored cross-platform matrix
cfee99e6 2026-07-28 fix(26-04): warn when a peer profile maps to neither provider nor model
a816533e 2026-07-28 fix(26-04): carry the symlink materialisation result into the case entry
0a75efd9 2026-07-28 test(26-04): scope the escape-admission assertion to the escaping item
```

The `PORT-*` row was authored 2026-07-28 citing only `Phase 26 (26-01, 26-03)`;
26-02 and 26-04 landed the same day and the row was never refreshed. The
ledger's last touch (`69ae4224`, 2026-07-29) was a Phase 22 re-grade that did
not revisit `PORT-*`.

**Therefore this lane does NOT build 26-02/26-04.** Building them again would be
manufacturing work to match a falsified brief (LANE-BRIEF, "verify the premise,
then act"). The lane's deliverable becomes: **independently verify that the
import half is real and not overclaimed**, run both directions of the quarantine
control, count the corpora actually executed, and refresh the ledger row.

The summaries were written by the lanes that built the code. Grading the code by
reading its author's summary is the tautology §3.2 warns about. Everything below
is measured from the code and from executed tests, not from the summaries.

## Instrument notes for this lane

- `/usr/bin/ls` does not exist on macOS (`/bin/ls`). First command of the lane
  hit this; caught because output was redirected to a file and read back.
- All counts in this file come from unproxied absolute-path tools with output
  redirected to a file and read with the Read tool, never through Bash stdout.

## Code surface measured (base b2ddf113)

```
crates/wcore-cli/src/migrate/  content.rs 867  gemini.rs 492  grok.rs 499
  hermes.rs 559  mod.rs 1555  openclaw.rs 569  provenance.rs 402
  quarantine.rs 1132  rollback.rs 468  select.rs 369     Σ 6912 lines
tests: migrate_hermes.rs  migrate_quarantine.rs  migrate_typed_dryrun.rs
       portability_hostile_corpus.rs
fixtures: tests/fixtures/portability-hostile/  tests/fixtures/portability-exec/
```

## Open at T0

- [ ] Does the quarantine gate run in BOTH directions (catch + pass-through)?
- [ ] How many hostile corpora actually EXECUTE (not skipped, not filtered out)?
- [ ] Is apply genuinely mutating, proven against a real home?
- [ ] Non-mutation of peer trees by digest.
