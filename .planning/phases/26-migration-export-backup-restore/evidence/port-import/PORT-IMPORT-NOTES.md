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

- [x] Does the quarantine gate run in BOTH directions (catch + pass-through)?
- [x] How many hostile corpora actually EXECUTE (not skipped, not filtered out)?
- [x] Is apply genuinely mutating, proven against a real home?
- [x] Non-mutation by digest.

## T1 — suites execute, nothing is ignored or filtered

`cargo test -p wcore-cli` over the four migrate binaries, hetzner `hz/port-import`
at `b6d7ee58`, output redirected to a file and read with the Read tool so the
`ignored` / `filtered out` fields survive (§3b):

```
migrate_hermes          7 passed; 0 failed; 0 ignored; 0 filtered out
migrate_quarantine     34 passed; 0 failed; 0 ignored; 0 filtered out
migrate_typed_dryrun   14 passed; 0 failed; 0 ignored; 0 filtered out
portability_hostile    23 passed; 0 failed; 0 ignored; 0 filtered out
                       -- 78 total, TESTRC=0
```

`#[ignore]` attribute count in the two files of interest: **0**. Known-positive
for the same grep: `#[ignore` IS found in `smoke_p0.rs`,
`release_manifest_pipeline.rs`, `f14_sigkill_recovery.rs`, `harness_tui_flow.rs`,
`acp_engine_turn.rs` — so the instrument was alive when it returned zero.

## T2 — 19 of 19 corpora execute; 0 unrun

`hostile_conservation_invariant_balances_across_every_corpus` with `--nocapture`
emits one `HOSTILE-OBSERVED` line per case: **19 lines, 19 declared cases, 0
unrun.** `1 passed; 22 filtered out` confirms the `--exact` filter matched a real
test (not §3.2 flavour (c)).

## T3 — source non-mutation, by digest, over every corpus

`nonmut.py` (committed beside this file) digests each corpus tree before and
after driving the REAL binary at it, covering relative path, mode, symlink
target and bytes.

**Instrument self-test first, three assertions:** digest stable across two reads;
goes red on a content change; goes red on a **mode-only** change — the third
being the one a content-only matcher would miss. All three green, so the 19 YES
results below are not free.

```
CORPORA-RUN=19 of 19  UNRUN=0  SOURCE-MUTATED=0 []
```

## T4 — the quarantine control, driven in both directions on the real binary

```
data-that-looks-executable (persona carrying shell SYNTAX — must pass through)
  Accounting: discovered=2 imported=2 quarantined=0
exec-disguised-as-data (executable claiming `trusted: true`, `auto_promote: true`,
                        `wayland_quarantine: exempt`, `kind: data` — must be caught)
  Accounting: discovered=2 imported=1 quarantined=1
  "Will be QUARANTINED — imported but INERT until you promote them (1)"
```

Both directions are REAL: the benign persona is genuinely imported (`imported=2`,
not silently dropped to reach `quarantined=0`), and five self-declared trust
claims in the payload do not talk the classifier out of containment.

## T5 — mutation testing: can the gates actually fail?

Passing tests do not establish a live gate (§3b-iii). I mutated the classifier on
hetzner and re-ran, restoring with `git checkout -- <path>` after each (dirty=0
verified each time).

| Mutation | Effect | Result |
|---|---|---|
| **M1** `classify_skill_body` → always `Data` (blinded) | catch direction must redden | **RED** — hostile catch case fails; `migrate_quarantine` 24 passed / **10 failed**, incl. `t1`, `t19` (live-negative), `t20` |
| **M2** `classify_skill_body` → always `Executable` (over-broad) | pass-through must redden | **RED in `migrate_quarantine`** — 29 passed / **5 failed**: `t2`, `t2b`, `t5`, `t23`, `t25`. **GREEN in the hostile suite** — 23 passed |
| **M3** `classify_data_kind` → always `Executable` | hostile pass-through must redden | **GREEN — 23 passed.** No effect at all |

So the quarantine contract IS load-bearing in both directions — catch via M1,
pass-through via M2 on the skill path. **But the hostile suite's own
pass-through case is not.**

### Instrument defect found in MY harness, and repaired here (§6b-ii)

My first mutation probe ran both binaries in one `cargo test` invocation. Under
M1 only **one** `test result` line appeared: cargo fail-fasts, so
`portability_hostile_corpus` **never ran** — and my probe printed
`hostile catch-direction: 0 failed`, which reads as "the mutation didn't affect
it" but means "it never executed". A participant that never started (§6a-i),
inside the harness hunting for exactly that class.

Repaired in the same lane rather than noted: one `cargo test` invocation per
binary, `--no-fail-fast`, and the probe now **reads back `running N tests` and
refuses to report at all if the binary did not run** (`UNRUN -- refusing to
report`). The repaired run is `MUTATION-M1-M2-HOSTILE.txt`; every row carries
`binary RAN 23 tests`. The old matcher would have reported `0 failed` for M1 —
the repaired one reports `1 FAILED`, which is the third assertion that proves the
repair does something.

## Findings

**F-PI-01 (MEDIUM) — one hostile case is a permanently-green gate.**
`hostile_data_that_merely_looks_executable_is_not_contained` sits under a header
reading `CLASSIFICATION, IN BOTH DIRECTIONS`, but **no mutation to any
classification predicate can redden it** (M2 green, M3 green). Its corpus is
`SOUL.md`, a persona, and the persona path is never content-classified — so the
case asserts a property that holds by construction. It is not wrong and not a
security hole (it fails safe, and the persona genuinely imports), but it is
counted as one of the two directions of the classification proof while having no
reachable fail state (§3b-iii). The direction is genuinely covered — by `t2` in
`migrate_quarantine`, which M2 does redden. Recommend the hostile suite's
pass-through case use a **skill** body with no directive, so the hostile set's
own classification section is live in both directions.

**F-PI-02 (LOW) — `classify_data_kind` is dead code whose comment claims
otherwise.** Its doc comment says it is "present as a function rather than a
comment so the breadth of the contract is stated in code and **can be
measured**". It has **zero production call sites** (`/usr/bin/grep -rn` over
`crates/`; known-positive `classify_skill_body` returns 5 production hits in the
same sweep). Its only reference is `quarantine.rs:1105`, a unit test asserting
that a constant function returns its constant. That is the
comment-asserts-a-property-the-code-does-not-implement class, in miniature.
Per AGENTS.md §3 pre-existing dead code is reported, not deleted.

Neither is CRITICAL or HIGH; both go to BACKLOG under §5 severity policy.
