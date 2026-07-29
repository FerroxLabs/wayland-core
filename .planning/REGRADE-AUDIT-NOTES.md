# REGRADE-AUDIT — working notes

Lane `lane/regrade-audit`, base `861d1b1a`. Started 2026-07-29.
Append-and-commit after every measurement. Do NOT save the writing up for the end.

## The question this lane answers

For each criterion in the six existing phase verdicts (21, 22, 27, 28, 29, 30):
**does the grade rest on an instrument now known to be broken?**

Two instrument defects are established:

- **D1 — nextest fd exhaustion.** Process-per-test holds ~2.9 pipe fds per concurrently
  running test; demand ~3x core count; crossing soft `RLIMIT_NOFILE` makes `fork`/`exec`
  fail. Measured 40 runs, `realfail = 0` in every one; worst run 96 exec failures / 0 real
  failures. **D1 manufactures FALSE REDS.** A grade it can corrupt is a NOT MET / FAILED
  that cites a red test run.
- **D2 — anti-vacuity guard never worked.** `.config/nextest.toml:37` sets
  `no-tests = "fail"`; installed cargo-nextest silently ignores it
  (`ignoring unknown configuration key`); `vx.toml` pins no version. **D2 manufactures
  FALSE GREENS** (a suite that ran nothing exits 0). A grade it can corrupt is a MET that
  cites a passing suite.

**Directionality is the whole audit.** D1 can only wrongly-fail. D2 can only wrongly-pass.
A grade whose evidence is "we never ran/built the thing" is reachable by NEITHER — no test
instrument participated.

## Outcome vocabulary (strict)

- **SOUND** — grade rests on evidence independent of D1 and D2. Name that evidence.
- **SUSPECT** — grade cites a run that could have been fd-exhausted or vacuous. Re-run on
  hetzner with fd budget raised, capture `TRY 1 XFAIL` vs `TRY 1 FAIL`.
- **UNGRADEABLE** — underlying evidence unrecoverable. Say so; do not guess either way.

## Running findings

### Phase 27 — read in full. PRELIMINARY: all grades SOUND, stand.

5 criteria: C1 PARTIAL, C2/C3/C4/C5 NOT MET. Every NOT MET rests on **work never
performed**, not on a red suite:

- C2 "Nothing is published" — readiness flags still `true` on a box with no browser binary;
  positive control present (operation failed `spawn camoufox: No such file or directory`).
- C3 "None of the four generation shapes was exercised. No MCP media-tool fixture was built."
- C4 "NOTHING WAS EXERCISED. No audio flowed on any machine." Verdict explicitly calls this
  an execution shortfall, not an environmental impossibility.
- C5 "Zero packaged smokes ran on zero platforms."

Neither D1 nor D2 can reach an absence-of-attempt. **Brief's prediction holds: 27's 0-of-5 is
about capabilities never built. STANDS.**

Note the one test-shaped claim in 27 (`2132/2132` after freeing disk) is a *pass* claim, so
D2-exposed in principle, but it is not load-bearing for any criterion grade — it was used to
clear 39 failures off 27's own account. Its own diagnosis (shared 1.8TB disk hit 100%,
`DispatchAdmission("... only 0 bytes are available")`) is a THIRD instrument defect, already
correctly identified by that lane.

### Phase 22 — read in full. PRELIMINARY: C1-C4 SOUND. C5 needs a look.

Original 2026-07-26 grading + a 2026-07-27 supersede block.
Current governing grades: C1 FAILED, C2 **PASSED** (both platforms), C3 FAILED, C4 PARTIAL,
C5 PARTIAL.

- C1 FAILED — source-level absence: no TUI surface, no host-protocol Goal events, no
  fixtures. Only 1 of 3 surfaces exists. Not a test result.
- C3 FAILED — source-level: five engines still return `ClimbOutcome`, `CouncilRunResult`,
  `WorkflowRunError`, caller-chosen `T`, and nothing. Adapter surface never built; no lane
  attempted 22-02 Task 3. Not a test result.
- C4 PARTIAL — `Dynamic`/`EventDriven`/`Manual` have no runtime enforcement. Source-level.
- C2 PASSED — this one is D2-EXPOSED in principle (a pass claim), BUT it carries its own
  falsification: "counting gate was falsified in the same run: a duplicated effect took it to
  13 and exit 1." A gate proven able to fail is not vacuous. Also driven against the shipped
  0.12.25 binary, not a test suite. Leaning SOUND — verify the falsification is real.
- C5 PARTIAL — **the one 22 grade with an instrument-shaped cause.** Windows M1-M5 never
  taken because "the reduce instrument needed for the cross-binary comparison died mid-build
  on a contended box (`EXIT=-1`)". That is a BUILD death under contention, not nextest fd
  exhaustion — different mechanism, and D1 is specifically nextest's spawn path. But
  contention-induced resource exhaustion is the same family. TO CHECK.

### Phase 30 — read in full. PRELIMINARY: all grades SOUND, stand.

C1 NOT MET, C2 NOT MET, C3 MET WITH STATED EXCEPTIONS, C4 PARTIAL.

- C1 NOT MET rests on **counted rows in a committed TSV**: `evidence/30-01/surface-truths.tsv`,
  148 data rows; operator-completeness unproven 148/148; peer-delta unproven 148/148.
  Universally-quantified criterion, so 100%-unproven fails outright. Row counting is not a
  test suite. Not D1-reachable.
- C2 NOT MET rests on `evidence/30-02/legs.tsv` — 15 legs, 9 RUN / 6 UNPROVEN, plus the
  confound (script emits `write_file`, a name only Hermes exposes; Core's is `Write`).
  The confound is a *design* fact about the script, re-derivable from the script text.
  Not D1-reachable.
- C3 MET WITH STATED EXCEPTIONS is D2-exposed in principle but carries an explicit tamper
  test (append one flattering sentence -> DETECTED) and a broken-reference refusal test
  (-> REFUSED, wrote nothing, named the offender). Both are known-negative controls in the
  same invocation. That is exactly the discipline D2 defeats when absent — here it is
  present. Leaning SOUND.
- C4 PARTIAL — mechanism proved both ways with a positive control ("a throwaway root
  generated at run time ACCEPTS a valid approval. Positive control, run first"). 14/14
  contract tests on hardware is D1/D2-exposed and small enough to re-run. TO CHECK.

30 also self-reports 5 instrument defects in its own lane, incl. rtk silently filtering
`git for-each-ref`. Consistent with the brief.

## Still to read

- 21-04-PHASE-VERDICT.md (340 lines)
- 28-04-PHASE-VERDICT.md (443 lines)
- 29-PHASE-VERDICT.md (304 lines) — four-of-four PARTIAL, highest suspicion of test-citation

## Method note

Every count that reaches the deliverable comes from `/usr/bin/grep`, `/usr/bin/git`,
`/usr/bin/wc` or `/usr/bin/env cargo` per LANE-BRIEF §3b. `rtk` strips `0 ignored` /
`0 filtered out`, which are the exact fields D2 detection needs.
