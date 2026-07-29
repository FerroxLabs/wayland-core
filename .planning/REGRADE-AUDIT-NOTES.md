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

---

# MEASUREMENT LOG — 2026-07-29

## M1. All six verdicts now read in full. Criterion inventory

| Phase | Criteria | Grades |
|---|---|---|
| 21 | 3 | C1 NOT-MET, C2 MET-W-STATED-EXC, C3 NOT-MET (amended DOWN 2026-07-26) |
| 22 | 5 | C1 FAILED, C2 **PASSED** (superseded up 07-27), C3 FAILED, C4 PARTIAL, C5 PARTIAL |
| 27 | 5 | C1 PARTIAL, C2/C3/C4/C5 NOT MET |
| 28 | 4 | C1/C2/C3 MET-W-STATED-EXC, C4 NOT MET (superseded to met-able 07-29 addendum) |
| 29 | 4 | all four PARTIAL |
| 30 | 4 | C1 NOT MET, C2 NOT MET, C3 MET-W-STATED-EXC, C4 PARTIAL |

**25 criteria total across the six phases.**

## M2. D1 SCOPING — the fd defect has a trigger condition, and it was never met

From `FLAKY-ROOT-CAUSE.md` (branch `lane/flaky-root-cause`, HEAD `6d1bf990`), read via
`git show 6d1bf990:.planning/FLAKY-ROOT-CAUSE.md`:

| `--test-threads` | peak runner fds | % of 1024 limit | outcome |
|---|---|---|---|
| 96 (= num-cpus, THE DEFAULT) | 299 | **29%** | **clean** |
| 192 | 569 | 56% | clean |
| 384 | hits cap | 100% | 13-81 exec-failures |

Model `peak ~ 2.9 x test-threads + 20`; the 1024 ceiling is crossed at **~346 test-threads**.
Root-cause lane: default concurrency **20/20 runs clean pre-fix**, and it could NOT reproduce
the reported failures at default concurrency in 37 attempts.

**So D1 fires only under (a) explicit `--test-threads` >~346, or (b) a lowered `ulimit -n`.**

**Measured across the six phase trees, unproxied `/usr/bin/grep`:**

```
exec failed                 files=0     <- the nextest exec-failure signature
os error 24                 files=0
cannot fork                 files=0
RLIMIT_NOFILE               files=0
```

Instrument alive: same sweep returned `nextest files=230`, `passed files=215`, `SIGKILL
files=54`, `filtered out files=98`. Non-zero on knowns, zero on the defect. **Not a dead grep.**

**Every `--test-threads` use inside the six phases is `=1`** — the CURE for D1, never the
trigger. Phases 22, 28, 29, 30 do not use the flag at all.

- 21-02-VACUITY-SUMMARY.md:129 `--test-threads=1` -> 2102 passed; 0 failed; 3 ignored
- 21-REVERIFICATION.md:442 `--test-threads=1`
- 27-FIXES-SUMMARY.md:222 `--test-threads=1`
- 27-GAPS-SUMMARY.md:203 `--test-threads=1` -> 551 passed; 0 failed

**CONCLUSION D1: no criterion in the six phases rests on an fd-exhausted nextest red.**

## M3. The ONE EMFILE red in scope — already correctly diagnosed, not a grade

`21-REVERIFICATION.md:446-459`. Two reds, both `file_watcher_notifier::tests::*`, both
`Io(Os { code: 24, message: "Too many open files" })`, taken while the box was at **load
146.59, 842 sessions, five lanes building**. Reported RED, attributed to host exhaustion,
**nothing ignored/re-gated/retried to green**. Also recorded that a PARALLEL run showed 14
failed, 12 of them `session journal writer lease is already held`, all cleared at
`--test-threads=1` — and stated "the parallel number is the one an unwary reader would
quote, and it is wrong."

These are bare `cargo test` (thread-parallel, one process), NOT nextest process-per-test, so
they are the EMFILE FAMILY but not D1's mechanism. And `file_watcher_notifier` is in no
Phase-21 criterion's evidence path. **Grade impact: none. Already handled correctly.**

## M4. D2 IS REAL AND IS CONFIRMED IN THE RECORD — and Phase 28 found it first

Line 1 of Phase 28's own raw capture
`28-.../evidence/28-04/hz-receipt.log`:

```
warning: in config file .config/nextest.toml, ignoring unknown configuration key: profile.default.no-tests
```

**The brief's D2 is confirmed verbatim in committed evidence.** Same file also appears in
`hz-crate.log`. Those are the ONLY 2 files carrying the string in the six trees.

**BUT the same log defeats the vacuity it warns about**, because nextest prints an executed
count regardless of the config key:

```
line 6 : Starting 49 tests across 2 binaries
line 57: Summary [0.120s] 49 tests run: 49 passed, 0 skipped
```

49 > 0, read back by the verdict (28-04 line 180: "49 tests run: 49 passed").

## M5. Four genuinely zero-executed runs found. Two benign, two real — all non-load-bearing

Search (control: `passed` -> 215 files, alive):

```
running 0 tests            files=9
0 tests run                files=11
test result: ok. 0 passed  files=3
Starting 0 tests           files=1
```

| Where | What | Load-bearing? |
|---|---|---|
| 27 `hetzner-targeted-tests.txt:24` | 3rd target of `cargo test -p wcore-types`; same invocation ran 137 + 5 | **No** — benign empty doc-test target |
| 29 `29-deny/acp-test2.txt:185` | explicitly `Doc-tests wcore_acp` -> `running 0 tests`; preceded by 4 passed | **No** — benign, crate has no doctests |
| 28 `kr07-linux-suites.log:1278` | `tests/otlp_local_test.rs` -> `running 0 tests`, **rc=0** | **No** — KR-07 sweep, not a grade input |
| 28 `kr07-linux-suites.log:1381` | `tests/bge_local_real.rs` -> `running 0 tests`, **rc=0** | **No** — same sweep |

The last two ARE true vacuous suites exiting 0. They sit in Phase 28's KR-07 evidence sweep,
not in any criterion's evidence.

## M6. Phase 28's cleanup lane already quantified and closed D2 — with a falsified gate

`28-CLEANUP-SUMMARY.md` ~215-270:

- **Measured 19 feature-gated + 25 platform-gated test binaries** that print `running 0 tests`
  and exit 0, "against a prior estimate of two". Largest: `wcore-mcp/tests/mcp_integration.rs`
  blanks **16 tests** without `--features test-utils`.
- Wrote `[profile.default] no-tests = "fail"` into `.config/nextest.toml` — **this is the
  origin of the `:37` line the brief calls inert.**
- **Falsified it, known-positive vs known-negative, on hetzner:**
  - zero-match: `cargo nextest run -p wcore-observability --test otlp_local_test` ->
    `Starting 0 tests across 1 binary` -> `error: no tests to run` **rc=4**
  - normal: `cargo nextest run -p wcore-observability --lib` -> `50 tests run: 50 passed` rc=0
- Stated explicitly: "**this is a nextest-only guarantee. Plain `cargo test` retains the
  hazard**, which is why the detector still exists."
- Detector carries a 4-positive/4-negative self-test AND the ancestral defect was
  **reintroduced into a copy** to prove the self-test can fail. That is the three-assertion
  discipline §6b-ii demands.

### OPEN QUESTION (the one worth re-measuring)

(a) nextest WARNS the `no-tests` key is unknown (M4), yet (b) nextest EXITS rc=4 on a
zero-match target (M6). Both are committed captures. These reconcile only if **fail-closed on
zero tests is cargo-nextest's BUILT-IN DEFAULT on 0.9.137**, making the config key redundant
rather than load-bearing.

If so, D2's blast radius is materially smaller than the brief assumes for nextest
invocations, and is confined to bare `cargo test` — which Phase 28 already says retains it.
**TO RE-MEASURE ON HETZNER.**

## M7. Phase 21 built anti-vacuity canaries on the leg that cannot self-pass

`21-02-VACUITY-SUMMARY.md`: the internal adversarial pass won one point and it is stated
plainly — the *widening* direction was already unamplifiable, so "a test that refuses a
widening request is largely theatre, and I have not built the canary on it." The canary is
built on the **narrowing differential**. Measured: "under the full revert-to-vacuous control,
the widening test still passes while the narrowing tests all go red."

Phase 21's verdict §5.2 also records a self-correction (VERIFICATION.md F-V4): the NO-CHANNEL
canaries as shipped at `1058965e` **could not go red**; repaired at `359ce2bf` and proved by
injecting a production file into the real tree -> `NO-CHANNEL CANARY TRIPPED`, green again on
removal.

**Phase 21 is the counter-example to the brief's worry: its instrument defects were found and
repaired IN-LANE, and its C3 grade was amended DOWNWARD as a result.**
