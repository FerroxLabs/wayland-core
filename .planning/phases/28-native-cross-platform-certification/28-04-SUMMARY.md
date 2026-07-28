---
phase: 28-native-cross-platform-certification
plan: "04"
subsystem: certification
tags: [F28-03, F28-04, receipt, finding-ledger, phase-verdict, independent-recomputation]
requires: ["28-01", "28-02", "28-03"]
provides:
  - "28-04-CERTIFICATION-RECEIPT.json — the phase-scoped signed receipt, eight bindings + skip policy + 63 findings"
  - "28-04-FINDING-LEDGER.md and evidence/28-04/findings.tsv — every finding re-scored and dispositioned"
  - ".planning/scripts/f28-verify-bindings.py — the independent recomputing verifier"
  - ".planning/scripts/f28-build-receipt.py — the receipt assembler"
  - "28-04-PHASE-VERDICT.md — the four-criterion verdict"
  - "crates/wcore-eval-scenarios/src/receipt.rs — CertificationReceiptV2 schema + CertificationVerifier"
affects: ["29-01", "29-04", "30"]
tech-stack:
  added: []
  patterns: ["independent-recomputation", "two-checkers-that-must-agree", "accounting-handoff", "authority-from-external-key"]
key-files:
  created:
    - .planning/phases/28-native-cross-platform-certification/28-04-CERTIFICATION-RECEIPT.json
    - .planning/phases/28-native-cross-platform-certification/28-04-FINDING-LEDGER.md
    - .planning/phases/28-native-cross-platform-certification/28-04-PHASE-VERDICT.md
    - .planning/phases/28-native-cross-platform-certification/evidence/28-04/findings.tsv
    - .planning/scripts/f28-verify-bindings.py
    - .planning/scripts/f28-build-receipt.py
    - crates/wcore-eval-scenarios/tests/f28_receipt_contract.rs
  modified:
    - crates/wcore-eval-scenarios/src/receipt.rs
    - .planning/scripts/f28-ledger.py
    - .planning/REQUIREMENTS.md
    - .planning/BACKLOG.md
decisions:
  - "F-28-02-002 was NOT re-scored to MEDIUM. A MEDIUM reading is arguable and would have opened the accept path and passed the gate; declining it is the whole point."
  - "OPEN is a recordable non-terminal disposition in the receipt schema, so the honest outcome is sayable rather than forcing a launder"
  - "The certification binds TWO candidates per scope rather than picking one and calling it 'the' candidate"
metrics:
  duration: "~1 session"
  completed: "2026-07-28"
status: complete
---

# Phase 28 Plan 04: Certification Receipt, Finding Adjudication and Phase Verdict — Summary

Bound the Phase 28 evidence into a phase-scoped signed receipt whose every claim is
independently recomputable from the raw artifacts, adjudicated all 63 findings under the
acceptance rule the panel settled, and graded the four Success Criteria verbatim: **three MET
WITH STATED EXCEPTIONS, one NOT MET.** The acceptance gate does **not** pass, and the receipt
says so rather than claiming otherwise.

**TERMINATION STATE: 2 — COMPLETE WITH STATED EXCEPTIONS / CRITERIA NOT ALL MET.** This is a
passing state for the plan and an honest one for the phase.

---

## 1. The SHA everything was built and verified at

| | |
|---|---|
| Lane branch | `lane/28-04` |
| Base / merge-base | `cf48b349d3aa84f85168511431b0a248fd50ded9` (captured once, quoted, never re-read from the branch name) |
| Verified commit | `3c656f2a2dc56b841885f438fa57037ab5065c7a` |
| Tree, asserted on the host | `808eda260c8d7f5334e145c58fe23e641de945cc` |
| Build host | `hetzner-dsm`, worktree `/root/wayland-p28-04` |

---

## 2. The receipt and its eight bindings, with the recomputed values

**`28-04-CERTIFICATION-RECEIPT.json`**, schema `wayland.cert.receipt` v2,
body digest `2037352cff1c2f2c8f8b35e59289ba73b514cd56977c8e22d599ed45e49e0fbb`.

| # | Binding | Bound | Recomputed from |
|---|---|---|---|
| 1 | candidate | 2 scopes: `matrix-linux-windows` at `32e2f57d`/`63ec0e6c`, `matrix-macos-rerun-and-soak` at `e4a3f5fc`/`6a494c99`; 5 per-target binary digests | the two candidate ledgers **and** the run/soak records that actually executed each binary |
| 2 | platform | linux 216/216, macos 216/216, windows 219/219 — **651 cells, 0 red, 0 skipped, 147 critical** | counting the raw cell list, with the macOS re-run **superseding** rather than accumulating |
| 3 | posture | 4 (fail-closed sandbox; `observation-blocked` NOT authorised; read-only workload; pre-registered bands) | `results.json`, `controls.json`, `soak.json`, `bands.json` |
| 4 | fixture corpus | 2, digested (`e5_cases.rs`, `e5_soak.rs`) | sha256 off disk |
| 5 | environment | 5 hosts incl. the GitHub macOS runner | `runs[]` and `families[]` |
| 6 | artifacts | 23, each digested | sha256 + byte length off disk |
| 7 | logs | 47, each digested | sha256 + byte length off disk |
| 8 | skip policy | 4 classes, **0 skipped cells, 0 skipped critical cases** | counting skipped cells in the raw list |

Plus the **finding ledger** as a first-class section: 63 findings with id, origin, subject,
inherited severity, Phase 28 re-score, contradicted criterion, disposition, rationale, owner,
backlog id, executable check and counter-evidence.

**Fields where recomputation was compared:** every candidate `commit`/`tree`, every binary
`sha256` against the record that ran it, all five per-family cell counts on all three families,
every artifact and log `sha256` and `bytes`, the skipped-cell count, the skipped-critical count,
the host set, the corpus digests, and all three A3 claims. Any disagreement is a rejection naming
the field.

### The phase-scoped key — and it is NOT a trust root

| | |
|---|---|
| Key id | `phase-28-certification-2026-07-28` |
| Public half | `Ks20+wo/p7Jeaa0c5DY4ex6ylMrIDfhs4TsWQ/6apIE=` |
| Fingerprint | `f0ef7d06c620b23c1ad84cc083d0a3a01c0c1ca7270a1cfdd5e46c9b050ed466` |

**It is bound to no release trust root, and none was created, rotated or published.** The scope
text lives inside the artifact itself, and a Rust test asserts it contains "not a release trust
root" and "not a seal", so a reader who never sees the verdict still cannot mistake it.

---

## 3. Both verifiers, and whether they disagreed

**They did not disagree.**

| Verifier | Result |
|---|---|
| Rust `CertificationVerifier` (hetzner) | `49 tests run: 49 passed, 0 skipped` — including two tests that read **the real artifact off disk** |
| Independent `f28-verify-bindings.py --verify` | **OK** — every digest and count recomputed from raw evidence |
| Whole crate regression (hetzner) | `405 tests run: 405 passed, 5 skipped` |
| `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` | **0 warnings, exit 0** |

The Rust test printed its real conclusion rather than a bare green:

```
verified 63 findings, gate_passed=false, unresolved CRITICAL/HIGH = ["F-28-02-002"]
```

**I checked that it had not skipped.** The test carries an early return, so I re-ran it with
`--nocapture` and read the output back — `running 2 tests` and the line above, not the
"nothing to check" branch. Then I **closed the hole**: the helper now FAILS when the phase
directory exists and the receipt does not, instead of passing.

Their A3 allowlists are cross-read from each other's source in the Python self-test, so the two
checkers cannot drift apart silently.

---

## 4. The demonstrations that the verifier can say NO

`evidence/28-04/verifier-rejections.log` — mutated copies of **this receipt**, each written next
to the real one so its relative evidence paths resolve.

| Mutation | Rejection |
|---|---|
| assert `zero_known_defects` | `F28V-OVERCLAIM` |
| assert `zero_findings` | `F28V-OVERCLAIM` |
| record a skipped critical case | `F28V-SKIPCRIT` + `F28V-SKIP` |
| flip a false claim to true | `F28V-CLAIM` — *recomputed False from the raw ledger* |
| drop one enumerated finding | `F28V-ENUM` |
| inflate a cell count by one | `F28V-PLATFORM` — *claims 217, recomputed 216* |
| rewrite a candidate commit | `F28V-CANDIDATE` |
| forge a log digest | `F28V-LOG` |
| flip one byte of the body | `F28R-DIGEST` |
| **control: the real receipt** | **all four gates OK** |

Also demonstrated: `--check-verdict` rejects a **narrowed** criterion (`F28V-VERBATIM`) and a
missing grade (`F28V-GRADE`); the strict ledger `--validate` fails with exactly one `F28L-002`
on the one open finding.

---

## 5. The finding ledger

**63 findings.** Full prose at `28-04-FINDING-LEDGER.md`; machine form at
`evidence/28-04/findings.tsv`.

| Phase 28 severity | n | | Disposition | n | | Origin | n |
|---|---|---|---|---|---|---|---|
| CRITICAL | 1 | | ACCEPTED | 28 | | matrix | 24 |
| HIGH | 8 | | DEFERRED | 18 | | candidate-resolution | 18 |
| MEDIUM | 41 | | FIXED | 9 | | certification | 8 |
| LOW | 13 | | DISPROVED | 7 | | carried-red | 6 |
| | | | **OPEN** | **1** | | contract / control / soak | 3 / 3 / 2 |

### The 7 A2 crossings — every one FIXED or DISPROVED

| id | Criterion | Severity | Disposition |
|---|---|---|---|
| `KR-05` | 1 | CRITICAL | **DISPROVED** — the wedge is a denial of service, not an elevation of privilege; in both wedged observations the product **refused to execute** |
| `KR-01` | 2 | HIGH | **DISPROVED** — the reap works, 12/12 serial, witness `[31360]` → `0 of 1` survivors |
| `F-28-02-001` | 1 | HIGH | **FIXED** — real product surface (`sandbox status\|exec`), macOS re-run 216/216 |
| `F-WR-01` | 1 | HIGH | **FIXED** — the test never reached its own assertion; rebuilt on proven primitives |
| `F-WR-02` | 1 | HIGH | **FIXED** — 16 zero-execution binaries guarded, falsified both directions |
| `F-KR-07` | 1 | HIGH | **FIXED** — test fix, not a product fix, stated plainly |
| `F-28-04-001` | 1 | HIGH | **FIXED** — the two macOS matrix members executed for the first time ever |

**Re-scores that moved a finding across the A2 line:** `KR-01` (inherited `known-red/non-gating`
→ HIGH) and `KR-05` (inherited `environment-quirk/non-gating` → CRITICAL), both pre-computed by
the 28-01 contract and both re-checked here.

**Downgrades: ZERO.** No finding was scored below the severity it arrived with, so no independent
downgrade review was required. Machine-checked, and the checker's self-test proves an *upward*
re-score is **not** caught — the ceremony sits exactly where the incentive is.

### The acceptance gate: NOT PASSED

> **`F-28-02-002` — HIGH — OPEN.** The stale AppContainer lease wedge is a persistent denial of
> service: a file nobody knows to look for permanently refuses all sandboxed execution, with a
> message that reads like a platform limitation.

At HIGH only FIXED and DISPROVED are available. It is CONFIRMED by control, so DISPROVED is out;
repairs are outside this plan's scope by design, so FIXED is out here.

**The re-score that would have passed the gate was declined deliberately.** A MEDIUM reading is
genuinely arguable under the contract's §3.1 bands — it contradicts no criterion, Windows passed
219/219 in the as-found state, and the wedged state was reached only by the control. **MEDIUM
opens ACCEPTED and DEFERRED and the gate passes.** Re-scoring downward so the accept path opens
is one of the three named forgeries, so the row keeps the severity Phase 28's own plan 02 gave
it. Recorded in the row so a later reader can reopen it deliberately.

### The 46 accepted and deferred findings

Each carries a rationale, a named owner and a `BL-F28-*` id **written into `BACKLOG.md` before
the ledger cited it**, and each is enumerated inside the signed receipt. 25 backlog entries were
added. Owners: Phase 29 (release acceptance) for the accounting-shaped ones, Phase 30 (hardening)
for the technical ones, the orchestrator for the retracted-belief cleanup, and Sean for the
Desktop contract corpus and the acceptance-rule counter-evidence.

---

## 6. The four Success Criteria — verbatim, with grades

Full text and evidence at `28-04-PHASE-VERDICT.md`; each criterion machine-checked against
ROADMAP.md.

| # | Criterion (verbatim) | Grade |
|---|---|---|
| 1 | *Native macOS, Linux, and Windows pass the required hostile platform matrix with no skipped critical case.* | **MET WITH STATED EXCEPTIONS** |
| 2 | *The 1,000-session/concurrent-child soak has no secret leak, orphan process, unbounded resource use, or unacceptable quality/performance delta.* | **MET WITH STATED EXCEPTIONS** |
| 3 | *Signed receipts bind exact candidate, platform, posture, corpus, environment, artifacts, logs, and skip policy.* | **MET WITH STATED EXCEPTIONS** |
| 4 | *Zero findings remain at every severity before acceptance.* | **NOT MET** |

**LIVE evidence, stated separately from in-process evidence.** All 651 matrix cells and all 3,000
soak sessions drove the real digest-bound `wayland-core` release binary on `hetzner-dsm`, the
certification Mac and `seandesktop`. `KR-01` was tested on real Windows hardware and disproved
with a host-side process witness. And **the two macOS members of the hostile platform matrix
executed on a real macOS host for the first time ever** — GitHub Actions run `30364529551` at
this plan's exact base SHA `cf48b349`, both `1 passed; 0 failed`, **no `skip:` line**.

**Exceptions, named rather than absorbed:** the coverage spans two candidates
(`F-28-04-004`); the macOS activeness observation is run-level (`F-28-04-007`); the two macOS
suites use no containment differential (`F-28-04-011`); the macOS orphan census is
non-authoritative (`F-28-04-005`); the soak workload is read-only (`F-28-04-006`).

---

## 7. The macOS question I was asked to judge rather than accept

**Are 0.05 s and 0.04 s substantive?** Yes, and for `hard_process_containment_macos` the speed is
the *evidence*. It runs `/bin/sh -c '/bin/sleep 45 & exit N'` under sandbox-exec twice and asserts
wall clock **< 20 s**. A backend that failed to reap leaves the detached `sleep` holding the
child's stdout pipe, so `execute` blocks until 45 s or the 30 s manifest timeout. **A non-reaping
backend physically cannot produce 0.05 s.** The bound is one-sided in the safe direction: load can
only make it slower, so load can only produce a false FAIL, never a false PASS.
`live_integrity_macos` is a matched pair whose inside-write half is the built-in control
separating "contained" from "failed to launch". Both are substantive.

**Is `1 filtered out` expected?** Yes, confirmed against source rather than assumed. Each binary
contains exactly two functions — the `#[ignore]`d `required_live_macos_*` case and the
always-running `zero_execution_guard` — and `--ignored` selects only the former. 2 = 1 run + 1
filtered, fully accounted for by a named function. The guard is skipped under nextest; the
workflow uses `cargo test`, so it was live.

**The residual, recorded as `F-28-04-011`:** neither forms a containment *differential*, so macOS
evidence for Criterion 1 rests on two different instruments rather than one.

---

## 8. Deviations from the plan

1. **`f28-ledger.py` was extended**, though it is not in `files_modified`. The plan's task-2 gates
   call `--validate`, `--check-completeness`, `--check-a2`, `--check-downgrades` and
   `--check-backlog-ids`, and none existed. Added with distinct `F28A-*` codes, each tripped in
   **both** directions by the self-test, plus proofs that an upward re-score and a repair-path row
   are **not** wrongly caught.
2. **`f28-build-receipt.py` was added.** The plan names the verifier but not the assembler; a
   receipt has to be built by something, and putting it in a script makes it re-runnable and
   reviewable rather than typed by hand.
3. **The v2 schema is a sibling type, not a mutation of `ReceiptBodyV1`.** Adding eight
   phase-level bindings to the per-run eval receipt would have forced every existing eval receipt
   to carry them and broken `receipt_contract.rs`, which this plan may not modify. The v1
   authority-derivation property is preserved verbatim.
4. **`OPEN` is a recordable disposition in the receipt schema.** The plan's behaviour list says a
   finding with no terminal disposition is rejected. Implemented at the *claim* level rather than
   by refusing the row, because a receipt that cannot represent an unresolved finding makes the
   honest outcome unsayable and becomes pressure to launder a HIGH. `zero_undispositioned_findings`
   is recomputed and false, and the gate does not pass — which is the same enforcement without the
   perverse incentive.

## 9. Self-passing shapes found in my OWN instruments, and fixed at cause

Recorded because this phase's whole subject is instruments that carry the defect they hunt, and
mine did it three times.

1. **The real-artifact Rust test early-returned into a green** when the receipt was absent —
   flavour B, the exact shape this phase inventoried across 16 binaries. Now FAILS when the phase
   directory exists and the receipt does not.
2. **A tamper fixture set `cells_red` to 0 on a family already at 0** — a no-op mutation asserting
   that nothing changes the digest. Every mutation now asserts it actually mutated before its
   verdict is trusted.
3. **`--check-verdict` searched only forward** from each quote and rejected a grade placed in the
   heading immediately above it. Adjacency has two directions; the window is now symmetric, and
   the check is still proved to fail on a narrowed criterion and a missing grade.

A fourth was a **rule that was too broad rather than too weak**: the empty-log check fired on a
zero-byte stderr capture whose emptiness is the good news. **Sharpened, not loosened** — it now
fires exactly when an empty file is *cited as finding evidence*, which is the only place one
could carry a claim.

---

## 10. What phases 29 and 30 inherit

**Phase 29 must consume `body.findings` in the signed receipt.** The receipt is an accounting
control, not a technical one: it records that a defect was known; it does not stop it reaching a
user. **If Phase 29 does not read that list, the accounting control has no consumer and this
acceptance rule is worth less than it looks.** Also inherited: the two-candidate split
(`BL-F28-TWO-CANDIDATES`), the acceptance rule's own counter-evidence (`BL-F28-C4`), and the
trust root this phase deliberately did not create.

**Phase 30** inherits the hardening backlog: `BL-F28-BWRAP-ETC`, `BL-F28-ACL-COST`,
`BL-F28-TEMP-SCRATCH`, `BL-F28-WEDGE-BASHPATH`, `BL-F28-MACOS-CENSUS`, `BL-F28-SOAK-WORKLOAD`,
`BL-F28-WIN-PARALLEL`, `BL-F28-VACUOUS-GREENS`, `BL-F28-COUNT-INFLATION`, `BL-F28-FLAVOUR-D` and
the rest.

**The one blocker: `F-28-02-002`, HIGH, OPEN.** It must be fixed or disproved before any phase can
claim the Phase 28 acceptance gate passed.

### Recorded unknowns

Whether a later restatement of Criterion 4 made in knowledge of the current severity policy
supersedes the planning-time decision (the dissent says it does, immediately); whether the
unproven-control corollary should have been applied, moving `KR-02` and `KR-03` across the A2
line; whether the `wedge-clearable` verdict generalises off `seandesktop` (not generalised);
whether a state-accumulating soak workload would show what the read-only one cannot.

---

## 11. What I did NOT do

- **Nothing was repaired.** No production defect fixed; no file under `crates/*/src` outside
  `wcore-eval-scenarios` touched — gate-checked against the **merge-base SHA**, never the branch
  name.
- **No existing test was modified.** `receipt_contract.rs` is byte-identical and still passes
  alongside the new contract in the same run.
- **No measurement was re-run to obtain a better number.** The matrix and soak are consumed as
  produced.
- **No test was weakened**, `#[ignore]`d, `#[allow]`ed, re-gated or deleted; no timeout raised.
  One clippy lint was resolved by **naming a type**, not by suppressing it.
- **No seal, no trust root, no release claimed.** No PR, no merge, no tag, no release, no issue
  closed, no retained evidence ref deleted, no credential supplied or printed.
- **`wcore-contract generate` was NOT run.** No contract change is needed, so **no seam request is
  filed.**
- **The shared fence (`crates/wcore-cli/src/{lib,main}.rs`) was not touched**, so there is nothing
  for the orchestrator to serialize for this lane.
- **The acceptance rule was not softened**, and **no fifth Phase 28 plan** was created.

## Self-Check: PASSED

All created files verified present on disk. All commits verified in `git log`: `a4d0c258`,
`1fbaa3b4`, `6e6b3754`, `9392ed45`, `3c656f2a`. Authoritative gates re-read from the retained
logs at `evidence/28-04/hz-clippy.log`, `hz-receipt.log`, `hz-crate.log`, and every figure quoted
above was read back from those logs rather than from an exit status.
