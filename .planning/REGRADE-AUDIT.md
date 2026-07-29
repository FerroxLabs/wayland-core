---
lane: regrade-audit
base: 861d1b1a716240165209336b1fa38d36f9445716
audited: 2026-07-29
phases: [21, 22, 27, 28, 29, 30]
criteria_total: 25
graded_by_broken_instrument: 0
grades_moved: 0
outcome_counts: {SOUND: 25, SUSPECT: 0, UNGRADEABLE: 0}
headline: >-
  ZERO of 25 criteria across the six graded phases rest on either known-broken
  instrument. No grade moves. The programme's bad-looking verdicts are bad
  product, not bad instruments.
correction_to_brief: >-
  D2 as stated is FALSE for cargo-nextest. The `no-tests = "fail"` key IS inert
  on 0.9.137 (confirmed verbatim), but fail-closed-on-zero-tests is that
  version's BUILT-IN DEFAULT: a zero-match run exits rc=4 regardless. The guard
  is REDUNDANT, not missing. D2's real blast radius is bare `cargo test` only.
fence_exposure: none
---

# REGRADE AUDIT — do our failing grades rest on broken instruments?

**Answer: no. Not one of them.**

Twenty-five criteria across Phases 21, 22, 27, 28, 29 and 30 were audited against the two
established instrument defects. **Zero were graded by a broken instrument. Zero grades move.**

That is not a comfortable result and it was not the expected one. The brief's hypothesis was
that some meaningful share of the programme's NOT METs were manufactured by a lying
instrument, and that we might therefore be rebuilding things that already work. **The evidence
does not support that hypothesis, and the reason is structural rather than lucky** — see §2.

---

## 0. Headline

| | |
|---|---|
| Criteria audited | **25** |
| Graded by a broken instrument | **0** |
| Grades that move | **0** |
| SOUND | **25** |
| SUSPECT | **0** |
| UNGRADEABLE | **0** |
| Re-measurements performed | 3 (nextest instrument characterisation; Phase 29 C4 absence claims; fd-trigger scoping) |

**One thing in the brief is wrong, and it is worth more than the audit result** — §3.

---

## 1. The two defects, and what each can physically do to a grade

**Directionality is the whole audit.** These two defects corrupt grades in *opposite*
directions, and neither can reach a grade that no test instrument produced.

| | D1 — nextest fd exhaustion | D2 — vacuous suite |
|---|---|---|
| Mechanism | process-per-test holds ~2.9 pipe fds per concurrently running test; crossing soft `RLIMIT_NOFILE` makes `fork`/`exec` fail; nextest reports `exec failed` | a suite exits 0 having executed zero tests |
| Manufactures | **FALSE REDS** | **FALSE GREENS** |
| Can corrupt | a NOT MET / FAILED **that cites a red test run** | a MET **that cites a passing suite** |
| Cannot corrupt | anything graded on work never performed, on source facts, or on counted evidence rows | same |

**The single most important structural fact about this programme's verdicts:** the
overwhelming majority of its NOT METs are graded on **work never performed** — "nothing was
published", "none of the four generation shapes was exercised", "no audio flowed on any
machine", "zero packaged smokes ran on zero platforms", "no lane attempted 22-02 Task 3". **A
test instrument cannot manufacture an absence of attempt.** D1 needs a red run to corrupt;
these grades cite no run at all.

---

## 2. D1 — scoped out by its own trigger condition

D1 is real, and it is not reachable from this programme's evidence.

### 2a. D1 has a trigger condition, and it is far above default concurrency

From `FLAKY-ROOT-CAUSE.md` (`lane/flaky-root-cause` @ `6d1bf990`), read unproxied via
`git show`:

| `--test-threads` | peak runner fds | % of 1024 soft limit | outcome |
|---|---|---|---|
| **96 (= `num-cpus`, the default)** | **299** | **29 %** | **clean** |
| 192 | 569 | 56 % | clean |
| 384 | hits the cap | 100 % | 13–81 exec-failures |

`peak ≈ 2.9 × test-threads + 20`; the 1024 ceiling is crossed at **~346 test-threads**.

**Confirmed independently on `hetzner-dsm` today:** 96 cores, `ulimit -Sn` 1024. `2.9 × 96 + 20
= 298`, matching the measured 299 — **29 % of the limit at default concurrency.** The
root-cause lane ran default concurrency **20/20 clean pre-fix** and could not reproduce the
reported failures at default in **37 attempts**.

**D1 therefore requires either an explicit `--test-threads` above ~346, or a lowered
`ulimit -n`.**

### 2b. Neither trigger was ever pulled in the six phases

Every `--test-threads` use inside the six audited phases is **`=1`** — which needs ~3 fds and
is the *cure* for D1, never the trigger:

| Phase | Site | Value |
|---|---|---|
| 21 | `21-02-VACUITY-SUMMARY.md:129` | `--test-threads=1` → 2102 passed; 0 failed; 3 ignored |
| 21 | `21-REVERIFICATION.md:442` | `--test-threads=1` |
| 27 | `27-FIXES-SUMMARY.md:222` | `--test-threads=1` |
| 27 | `27-GAPS-SUMMARY.md:203` | `--test-threads=1` → 551 passed; 0 failed |

**Phases 22, 28, 29 and 30 do not use the flag at all.**

### 2c. The defect signature is absent from the record — with a live-instrument control

Unproxied `/usr/bin/grep -rl` across all six phase trees:

```
exec failed                 files=0     <- THE nextest exec-failure signature
os error 24                 files=0
cannot fork                 files=0
RLIMIT_NOFILE               files=0
```

**Per §3b-i, an absence is worthless without a live instrument. Same sweep, same invocation:**

```
nextest                     files=230   <- non-zero: grep is alive
passed                      files=215
SIGKILL                     files=54
filtered out                files=98
0 ignored                   files=107
```

The instrument answers non-zero on five knowns and zero on the defect. **Not a dead grep.**

**Concept, not keyword** (§3b-i.3): I swept the whole exhaustion *family*, not just one string
— `EMFILE`, `Too many open files`, `os error 11`, `Resource temporarily unavailable`,
`cannot fork`. That found exactly one cluster, below.

### 2d. The one EMFILE red in scope was already diagnosed correctly

`21-REVERIFICATION.md:446-459`. Two reds, both `file_watcher_notifier::tests::*`, both
`Io(Os { code: 24, message: "Too many open files" })`, taken while the box was at **load
146.59 with 842 sessions and five lanes building**. They were **reported RED and attributed to
host exhaustion** — nothing ignored, re-gated, retried to green or given a longer timeout. The
same document also records that a *parallel* run showed 14 failed, 12 of them `session journal
writer lease is already held`, all clearing at `--test-threads=1`, and states plainly: *"the
parallel number is the one an unwary reader would quote, and it is wrong."*

Two reasons this changes no grade: these are **bare `cargo test`** (thread-parallel, one
process), so not D1's process-per-test spawn mechanism at all; and `file_watcher_notifier` is
in **no Phase 21 criterion's evidence path**.

**D1 verdict: no criterion in the six phases rests on an fd-exhausted red.**

---

## 3. D2 — the brief is wrong, and the correction matters

This is the most consequential finding in the lane, and it cuts against the brief.

### 3a. The inert key is real — confirmed verbatim in committed evidence

Line 1 of Phase 28's own raw capture, `28-*/evidence/28-04/hz-receipt.log`:

```
warning: in config file .config/nextest.toml, ignoring unknown configuration key: profile.default.no-tests
```

**The brief's D2 premise is confirmed on that half: the key IS ignored.**

### 3b. But fail-closed-on-zero-tests is nextest 0.9.137's BUILT-IN DEFAULT

Re-measured today on `hetzner-dsm` in an **isolated scratch crate** — deliberately outside the
workspace, so the instrument question is answered without any repo variable. cargo-nextest
0.9.137 (75ddba7e9). Every case carries its control:

| # | Case | rc | Output |
|---|---|---|---|
| **A** | zero-match filter, **no config file at all** | **4** | `Starting 0 tests` → `error: no tests to run` |
| **B** | control — filter that DOES match | **0** | `Starting 1 test` |
| **C** | zero-match **+ `[profile.default] no-tests = "fail"`** | **4** | warns key unknown, **still refuses** |
| **E** | zero-match + top-level `no-tests` | **4** | warns key unknown, **still refuses** |
| **F1** | **blank binary under bare `cargo test`** | **0** ← **HAZARD** | `running 0 tests` / `test result: ok. 0 passed` |
| **F2** | same blank binary under **nextest** | **4** | `error: no tests to run` |
| **G** | workspace run, one blank binary among others | **0** | `2 tests run: 2 passed` — correct |
| **H** | known-negative — a genuinely failing test | **100** | `FAIL … 1 failed` |

Case A is the load-bearing one: **with no config whatsoever, nextest already exits 4.** Case C
proves the key is inert *and* irrelevant. Case H proves the three states are distinguishable
(0 pass / 4 no-tests / 100 real failure) — a healthy instrument, not a one-bit one.

> **So: the `no-tests = "fail"` guard is REDUNDANT, not missing.** The brief's inference —
> *"the suite passed" may mean "the suite ran nothing"* — **is false for every nextest
> invocation on this toolchain.** It is true only for bare `cargo test` (F1).

That is exactly what Phase 28's cleanup lane already wrote, and it was right:
*"this is a nextest-only guarantee. Plain `cargo test` retains the hazard, which is why the
detector still exists."*

### 3c. Vacuous runs in the record: four found, all characterised, none load-bearing

Search across the six trees (control: `passed` → 215 files, alive):

| Where | What | Load-bearing? |
|---|---|---|
| 27 `evidence/27-gaps/gates/hetzner-targeted-tests.txt:24` | 3rd target of `cargo test -p wcore-types`; **same invocation ran 137 + 5** | **No** — benign empty doc-test target |
| 29 `evidence/29-deny/acp-test2.txt:185` | labelled `Doc-tests wcore_acp`; preceded by `4 passed` | **No** — benign, crate has no doctests |
| 28 `evidence/28-kr07-suites/kr07-linux-suites.log:1278` | `tests/otlp_local_test.rs` → `running 0 tests`, **rc=0** | **No** — KR-07 inventory sweep, not a criterion input |
| 28 `…:1381` | `tests/bge_local_real.rs` → `running 0 tests`, **rc=0** | **No** — same sweep |

The last two are **genuine** vacuous suites. They sit in Phase 28's KR-07 *evidence sweep*,
whose entire purpose was to find them.

### 3d. Phase 28 had already quantified and closed D2 — with a falsified gate

`28-CLEANUP-SUMMARY.md`:

- **Measured 19 feature-gated + 25 platform-gated blank test binaries**, "against a prior
  estimate of two". Largest: `wcore-mcp/tests/mcp_integration.rs` blanks **16 tests**.
- **Wrote** the `[profile.default] no-tests = "fail"` line — *this is the origin of the
  `.config/nextest.toml:37` the brief calls inert.*
- **Falsified it known-positive against known-negative** on hetzner: zero-match → `error: no
  tests to run` **rc=4**; normal run → `50 tests run: 50 passed` rc=0.
- Built a detector with a **4-positive / 4-negative self-test**, then **reintroduced the
  ancestral defect into a copy** to prove the self-test can fail — the third assertion §6b-ii
  demands.

**D2 verdict: real for bare `cargo test`, already found and bounded by Phase 28, and
load-bearing for no criterion in the six phases.**

---

## 4. The 25 criteria

`OG` = original grade. All evidence bases were read in the verdicts and spot-verified.

### Phase 21 — Child Authority and Budget Inheritance

| # | Criterion | OG | Outcome | Re-graded | Evidence independent of D1/D2 |
|---|---|---|---|---|---|
| 1 | child cannot widen ANY of 11 restrictions | NOT MET | **SOUND** | no change | Source fact: `build_tool_registry(&["Bash","Write"], …)` registers Bash without consulting the parent, confirmed by the product's **own unit test** at `spawner.rs:4357`. 6 of 11 dimensions `NO-CHANNEL`/`NOT-EXPRESSIBLE`. 4-way panel unanimous NOT-MET. No test run is cited as red. |
| 2 | nested lifecycle attributable to correct parent | MET w/ EXC | **SOUND** | no change | Upward grade, so D2-exposed in principle — but it rests on **positive observations**: two distinct `parent_call_id` values on the shipped `--json-stream` wire, each sibling's result under exactly one. A vacuous run produces no observations to report. |
| 3 | standalone/host corpora prove equivalent enforcement | NOT MET | **SOUND** | no change | Counted field `child_turns=0` on all 12 decisive standalone rows + product fact (`ToolConfirmer::check_for` denies when stdin is not a tty). **Already amended DOWN** 2026-07-26 for a self-passing-gate defect caught in-lane — a different defect class, correctly handled. |

### Phase 22 — Supervision, Durable Goals, Fleet Loops

| # | Criterion | OG | Outcome | Re-graded | Evidence independent of D1/D2 |
|---|---|---|---|---|---|
| 1 | three surfaces observe identical state | FAILED | **SOUND** | no change | Source absence: no TUI surface, no host-protocol Goal events, no fixtures. 1 surface of 3 exists. |
| 2 | fleet claims survive kill/restart | **PASSED** | **SOUND** | no change | Live drive of the shipped 0.12.25 binary on Linux **and** Windows; effects 12/12/12. **Gate falsified in the same run** — a duplicated effect took it to 13 and exit 1. A gate proven able to fail is not vacuous. |
| 3 | one canonical terminal transition | FAILED | **SOUND** | no change | Source fact: the five engines still return `ClimbOutcome`, `CouncilRunResult`, `WorkflowRunError`, a caller-chosen `T`, and nothing. Adapter surface never built; **no lane attempted 22-02 Task 3.** |
| 4 | bounded session-local loops | PARTIAL | **SOUND** | no change | Source fact: `Dynamic`/`EventDriven`/`Manual` have no runtime enforcement. |
| 5 | journal compatibility proved or migrated | PARTIAL | **SOUND** | no change | Windows M1–M5 **never taken** — the reduce instrument died mid-build (`EXIT=-1`) on a contended box. That is a *build* death, not nextest's spawn path, and the grade cites **no run at all**. See the note below. |

> **Note on 22-C5 — the only grade in the audit whose incompleteness was caused by machine
> contention.** It is still SOUND under the strict definition (SUSPECT requires a *cited run*
> that could have been corrupted; this cites the absence of one, which is the honest
> direction). But it is the **cheapest grade in the programme to close**: `C:\p22` is already a
> detached worktree at the right commit with `wayland-core.exe` built, and only
> `examples/p22_reduce.rs` needs compiling on a quiet box.

### Phase 27 — Multimodal, Browser, Generation, Voice

| # | Criterion | OG | Outcome | Re-graded | Evidence independent of D1/D2 |
|---|---|---|---|---|---|
| 1 | one bounded validated attachment/document intake | PARTIAL | **SOUND** | no change | Two RED gates reported RED (`media_intake` absent from `attachments.rs`/`channel_media.rs`; `supports_document_input` not added). Live single-variable capture proved the vision gate. |
| 2 | browser/CUA/web publish live readiness | NOT MET | **SOUND** | no change | "Nothing is published." Flags still `true` on a box with **no browser binary**, with a positive control — the operation failed `spawn camoufox: No such file or directory`. |
| 3 | four generation shapes consistent | NOT MET | **SOUND** | no change | **"None of the four generation shapes was exercised."** No MCP media-tool fixture was ever built. |
| 4 | streaming voice interruption/cancellation | NOT MET | **SOUND** | no change | **"NOTHING WAS EXERCISED. No audio flowed on any machine."** Verdict names it an execution shortfall, not an environmental impossibility. |
| 5 | deterministic corpora + packaged smokes on 3 OSes | NOT MET | **SOUND** | no change | **"Zero packaged smokes ran on zero platforms."** Every Linux measurement came from a build-tree binary. |

> **The brief predicted Phase 27's 0-of-5 would stand because it is about capabilities never
> built. That prediction is correct on all five.** No instrument participated in any of these
> grades.

### Phase 28 — Native Cross-Platform Certification

| # | Criterion | OG | Outcome | Re-graded | Evidence independent of D1/D2 |
|---|---|---|---|---|---|
| 1 | hostile platform matrix, no skipped critical case | MET w/ EXC | **SOUND** | no change | 651 cells **recounted from the raw cell list**, not read from a summary. Every sandbox cell required a **containment differential**. macOS GHA run 30364529551 checked for **`skip:` lines** (flavour b) and accounted for `1 filtered out` exactly (flavour c). **This phase performed the D2 read-back manually.** |
| 2 | 1,000-session soak: no leak, orphan, unbounded use | MET w/ EXC | **SOUND** | no change | 3,000/3,000 sessions. **Every observable carries a positive control that was CAUGHT** — canary planted in 6 channels and detected in all 6; a deliberately orphaned product process FOUND by every census; a growing lane FLAGGED. A missed control makes the observable VOID, not green. Bands pre-registered at `1dea6437` before any session existed. |
| 3 | signed receipts bind 8 dimensions | MET w/ EXC | **SOUND** | no change | Two **independent** checkers (Rust + Python) that must agree; verifier **seen to say NO** on 9 distinct mutations of this very receipt. `49 tests run: 49 passed` — and the raw log carries `Starting 49 tests` on line 6, so 49 > 0 is readable despite the D2 warning on line 1. |
| 4 | zero findings remain at every severity | NOT MET | **SOUND** | no change | Rests on **one ledger row** (`F-28-02-002` OPEN at HIGH), not on any test run. Verdict explicitly **declined** the MEDIUM re-score that would have opened the accept path. |

> **28-C4 has already moved — on repair evidence, not instrument evidence.** The 2026-07-29
> addendum records `F-28-02-002` repaired (`15821c03` + `3f3f93dc`) and **independently
> adjudicated FIXED by a lane that did not author the repair**, with a superseding receipt
> (`gate_passed=true`). The original signed receipt was correctly left byte-identical. **This
> is a grade moving the right way for the right reason and is outside this audit's scope.**

### Phase 29 — Supply Chain and Release Integrity

| # | Criterion | OG | Outcome | Re-graded | Evidence independent of D1/D2 |
|---|---|---|---|---|---|
| 1 | clean-room builds verify provenance/SBOM/policy/sigs/repro | PARTIAL | **SOUND** | no change | The blocking half is **cargo-deny exit 5** — `advisories FAILED, bans ok, licenses FAILED`. cargo-deny is not a test runner and has no per-test spawn path; D1 cannot reach it. Open HIGH F29-02-H1 is a **dependency-graph fact** (`wcore-tools` → quick-xml 0.39.4 directly, default-on `doc-extract`). |
| 2 | install/update verify identity, rollback, revocation, rotation | PARTIAL | **SOUND** | no change | Blocked by absence of a **real trust root** and a **real published manifest asset** (measured: v0.12.25 publishes 7 assets, none a manifest). Rollback proved live against the real public GitHub API. |
| 3 | tampered artifacts/manifests/receipts/plugins/keys rejected | PARTIAL | **SOUND** | no change | 12 paired rows, every one `control=ACCEPTED :: mutated=REFUSED`, **pairing enforced by the type** — a case without its control cannot be constructed. The gap is a source fact: `INDEX_PUBKEY_HEX` is still all-zeros. |
| 4 | packaging/deployment/rollback/acceptance stay separate | PARTIAL | **SOUND** | **re-measured today — confirmed** | See below. |

**29-C4 was the one criterion whose failing half rested entirely on absence claims about
workflow files — the §3b-i self-passing class. I re-measured all three today.**

Controls first, in the same invocation (all must be non-zero):

```
files in .github/workflows/ : 11
lines matching runs-on      : 26
lines matching jobs:        : 14
```

Then the claims:

```
C4.1 jobs declaring "environment:"     : 0     (grep -rnE '^[[:space:]]*environment:')
C4.2 any mention of rollback (-i)      : 0
C4.3 ledger verbs in release.yml       : 0     (wayland-release|state-append|state-verify|release-manifest)
```

Concept sweep rather than one keyword: `approval` 0, `approve` 0, `protected` 0, `reviewers`
0, `workflow_dispatch` 20. An unanchored `-i environment` returned **6** — I read all six and
**every one is prose inside a comment or a string** (`"environment-specific"`, `"The crash is
environmental to…"`), not a job-level gate. **All three absence claims CONFIRMED.**

> **A live demonstration of the trap, from this lane's own work:** my first attempt used
> `/usr/bin/ls .github/workflows/*.yml | wc -l` and got **0** — while `grep -r` on the same
> directory was returning matches. The glob had silently expanded to nothing. **The zero was my
> instrument dying, not an empty directory.** I caught it only because I had demanded a
> non-zero control in the same invocation. This is the eleventh-plus instance of the class and
> the reason §3b-i exists.

### Phase 30 — Continuous Scorecard and Frontier Review

| # | Criterion | OG | Outcome | Re-graded | Evidence independent of D1/D2 |
|---|---|---|---|---|---|
| 1 | every surface has 6 refreshed truths | NOT MET | **SOUND** | no change | **Counted rows in a committed TSV**: `surface-truths.tsv`, 148 data rows; operator-completeness unproven **148/148**; peer-delta unproven **148/148**. A universally-quantified criterion fails outright at 100 % unproven. Row counting is not a test suite. |
| 2 | 3 tools complete 5-dimension trials | NOT MET | **SOUND** | no change | `legs.tsv` — 15 legs, **9 RUN / 6 UNPROVEN**. The confound is a **design fact re-derivable from the script text**: it emits `write_file`, a name only Hermes exposes; Core's is `Write`. Two dimensions have **zero legs**. |
| 3 | published claims match evidence, no superiority language | MET w/ EXC | **SOUND** | no change | Upward grade, but carries **both controls in the same invocation**: tamper test (append one flattering sentence → **DETECTED**) and broken-reference publish (→ **REFUSED**, wrote nothing, named the offender). 12 distinct refusal rules actually fired across a 24-row paired corpus. |
| 4 | no reserved action without Sean's approval | PARTIAL | **SOUND** | no change | 14/14 contract tests on hardware **with the positive control run first** (a throwaway root generated at run time **ACCEPTS** a valid approval — so the verifier is not one that merely refuses everything). `ls-remote` carried a falsification control proving the check can answer YES. |

---

## 5. What I did NOT do

- **I did not re-execute the 25 criteria's underlying suites.** The method was: characterise
  each defect's trigger condition, prove the trigger was never pulled, and search the record
  for each defect's signature with live-instrument controls. That is sound for the question
  asked — *does this grade rest on a broken instrument* — but it is documentary plus
  instrument-characterisation, **not a full re-run of the programme**. A grade wrong for some
  *third* reason would not be caught here.
- **I did not re-run the fd-budget harness.** `scripts/fd-budget.sh` on `lane/flaky-root-cause`
  was not needed: no audited grade cites a run at a concurrency or fd budget where D1 can fire,
  so raising the budget would change no number. Re-running it would have produced a green that
  proved nothing about these grades.
- **I did not touch `crates/`, `.github/workflows/`, or any phase verdict.** Workflow files
  were **read only**.
- **I took no reserved action** — no merge, no PR, no tag, no release, no issue closure, no
  `wcore-contract generate`.
- **I did not upgrade a single grade.** No grade moved in either direction.

## 6. Adversarial pass against my own conclusion

A 0-of-25 result is exactly the shape a lazy audit produces, so I argued against it:

1. *"You proved D2 fails closed under nextest, but many grades cite bare `cargo test`, where it
   does not."* — Answered directly rather than by inference: I searched every tree for the
   zero-executed signatures, found **exactly four**, and characterised all four (§3c). None is
   a criterion input. This is the direct check, not an argument.
2. *"The five upward (MET) grades are the D2-exposed ones — did you actually check each?"* —
   Yes, individually: 21-C2, 22-C2, 28-C1/C2/C3, 30-C3. **Every one carries a control that was
   observed firing** — a caught canary, a falsified counting gate, a detected tamper, a
   refused publish, an accepting positive root. A vacuous suite cannot produce a *caught
   control*; it produces silence.
3. *"Isn't 'the NOT METs are all about work never done' just deference to the verdicts?"* — It
   is checkable and I checked the strongest instances: 22-C3's five distinct return types,
   29-C4's three workflow zeros (re-measured today), 30-C1/C2's TSV row counts, 27's
   never-run legs. These are source and data facts, not test outcomes.
4. **Where I lost:** I cannot rule out that a grade is wrong for a reason *neither* defect
   describes. Phase 21-C3 is the existence proof — it was over-graded by a **third** defect
   (an equivalence assertion that could not fail) and was only caught by a dedicated
   verification pass. **That class is not covered by this audit and is not measured here.**

---

## 7. What this means for the plan

**Nothing in the current plan should be dropped on instrument grounds.** The bad grades are
bad product:

- **Phase 27** is unmet because voice, generation, browser readiness and packaged smokes were
  **never exercised**. That work is still to do. The verdict's own top recommendation stands:
  land `SEAM-REQUESTS/27.md`, fix the two-word `[browser]` vs `[browser.policy]` string at
  `wcore-browser/src/tool.rs:499`, and run voice on `seandesktop`.
- **Phase 22** is unmet because the adapter surface over the five loop owners (22-02 Task 3)
  was never attempted. Criterion 3 is the hard one and no lane has touched it.
- **Phase 30** is unmet because 2 of 5 dimensions never ran and the other 3 are confounded by a
  tool-name dialect mismatch. Its own KEY-08 (per-tool dialect compilation) **needs no
  credential and no authorisation** and is named the cheapest, most consequential item.
- **Phase 29** is PARTIAL across the board pending a real trust root and a real published
  manifest — both Sean-reserved or release-pipeline items, not engineering gaps.

**The one thing that IS worth acting on from this audit** is the D2 correction in §3b: the
programme has been treating `no-tests = "fail"` as a missing guard. It is a *redundant* one.
The real, still-open hazard is **bare `cargo test` against a targeted blank binary** (F1, rc=0)
— 19 feature-gated and 25 platform-gated binaries are exposed, already inventoried by Phase 28
and filed as `BL-F28-VACUOUS-GREENS`. **Routing those invocations through nextest closes it
generically.**

---

## 8. Fence exposure

**None.** Versus `861d1b1a`, this lane's diff touches `.planning/REGRADE-AUDIT.md` and
`.planning/REGRADE-AUDIT-NOTES.md` only. `crates/wcore-cli/src/lib.rs` and
`crates/wcore-cli/src/main.rs` are untouched; `crates/` is untouched; `.github/workflows/` is
untouched (read-only access only). The hetzner measurements were made in
`/root/regrade-audit-scratch`, a throwaway crate outside any repo worktree.

---

_Audited 2026-07-29, lane `lane/regrade-audit`, base `861d1b1a`._
_Every number above came from `/usr/bin/grep`, `/usr/bin/git`, `/bin/ls`, `/usr/bin/wc` or
`/root/.cargo/bin/cargo` — never through `rtk`, which strips `0 ignored` / `0 filtered out`._
