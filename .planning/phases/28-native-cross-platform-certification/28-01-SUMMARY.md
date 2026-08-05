# 28-01 SUMMARY — certification contract, candidate resolver, E5 matrix generator

**Plan:** `28-01` (wave 1, `depends_on: []`) — requirements **F28-01**, **F28-04**.
**Lane branch:** `lane/28-01`, HEAD `eaca8e95`.
**Termination state: 2 — COMPLETE WITH STATED EXCEPTIONS.** Contract, resolver and generator
exist; the resolver produced a candidate ledger; the generator's contract tests pass on Linux.
The exceptions are real and are stated in §4: **0 of 6 per-target binary digests could be
bound**, and the resolved candidate is **provisional**.

---

## 1. Verdict per Success Criterion

Phase 28's own four criteria are not this plan's to close — this plan builds what closes them.
Graded against the plan's own twelve success criteria instead, with the phase criteria they
serve named.

| # | Plan success criterion | Verdict | Evidence |
|---|---|---|---|
| 1 | Contract quotes the four criteria, the standing policy and the adopted rule with three amendments verbatim; records the 4-0 split and points at the dissent (F28-04) | **MET** | `f28-ledger.py --check-contract` extracts **13 anchors from their original sources** (ROADMAP.md, REQUIREMENTS.md, decision-rationale.txt) and asserts each appears in the contract. Not a self-grep. |
| 2 | Severity rubric defines re-scoring, keeps inherited severity as provenance only, works both calibration examples (F28-04) | **MET** | Contract §3.2, §3.3. `F28L-006` rejects a finding with no re-score; proved red by stripping KR-01's re-score. |
| 3 | Exactly four skip classes, each machine-checkable; a critical cell has none (F28-01, F28-04) | **MET** | Contract §4. In Rust an unclassified skip is **unrepresentable**, not merely rejected — `SkipEvidence`'s variants *are* the classes. `F28M-002`/`F28M-003` cover the TSV. |
| 4 | Observation-blocked requires a run-time control and rejects inherited lore; no gate has "SSH sandbox reds are artifacts" as a passing condition (F28-01) | **MET** | Contract §4.2, `MatrixError::ObservationBlockedCitesDocument`, `F28M-004`. Proved red against **the intel file that reports in the product's favour**. |
| 5 | Sandbox activeness rule stated in the contract and enforced in the generator's types (F28-01) | **MET** | Contract §5. `ActivenessEvidence` has no "no violation observed" variant; `SandboxPass::new` refuses `NotMeasured` and refuses empty `Observed`. Three contract tests. |
| 6 | Every known-red has origin, inherited severity, re-score, contradicted criterion, dispositions; the two A2 crossings have a closed accept path proved by `--check-rescoring` (F28-04) | **MET** | `evidence/28-01/known-red.tsv`, 6 rows. `F28L-015` fires when KR-01 or KR-05 loses its crossing; proved red. |
| 7 | Resolver binds commit to tree, digest-or-unbindable per target including arm64 macOS, surfaces off the binary, fails closed (F28-01) | **MET** | `--self-test` refuses a ref, an abbreviated sha, an unbound tree, a mismatched capture, a truncated capture, an omitted target, a reasonless unbindable, a bad digest, and a missing phase artifact — each with a fixture that also proves the rule does not over-fire. |
| 8 | Both mismatch classes emitted as findings, never dropped (F28-01) | **MET (with a measurement caveat, §5)** | 17 findings emitted. **Zero** claimed-but-absent, 10 present-but-unclaimed, 7 attribution-weak. |
| 9 | Generator crosses nine verbatim dimensions × three families × resolved surfaces and REJECTS four defect shapes, each proved by a test that trips it (F28-01) | **MET** | 28/28 contract tests on Linux hardware. Every rejection asserted in **both** directions. |
| 10 | All three mandatory cells present, critical, unskippable (F28-01) | **MET** | `all_three_mandatory_cells_are_present_critical_and_unskippable`, `removing_a_mandatory_cell_fails` (×3), `downgrading_a_mandatory_cell_off_critical_fails`. |
| 11 | Clippy clean at `-D warnings`, contract tests pass on Linux, both logs retained and content-gated | **MET** | `hz-clippy.log` `EXIT=0`; `hz-matrix.log` `28 tests run: 28 passed, 0 skipped`. Both at `553e848b`, SHA asserted **on the host before any build step**. |
| 12 | No production file outside `wcore-eval-scenarios`; no test weakened; no 24-27 surface named | **MET** | `git diff --name-only 32e2f57d..HEAD -- crates/` returns exactly three `wcore-eval-scenarios` paths. §7. |

---

## 2. Gate results — real numbers

**Local (Mac, source-level only; no cargo beyond `fmt`): 19 run, 19 passed, 0 failed.**

**Authoritative (hetzner `hetzner-dsm`, worktree `/root/wayland-p28`, commit `553e848b`,
tree `133d3e71`, SHA asserted on the host before any build step):**

| Gate | Result |
|---|---|
| `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` | **0 warnings, EXIT=0** |
| `cargo nextest run -p wcore-eval-scenarios --test e5_matrix_contract --no-fail-fast` | **28 run, 28 passed, 0 skipped, 0 failed** |
| `cargo fmt --all -- --check` (Mac) | clean |

Logs retained at `evidence/28-01/hz-clippy.log` and `hz-matrix.log`, and gate-checked for the
test-binary name (28 occurrences) so a gate cannot pass on an empty or unrelated log.

### Every gate was proved able to go red

Not asserted — executed. Each mutation below was applied to the **real** artifact and rejected,
and the unmutated artifact was re-run immediately after and accepted:

| Mutation | Rejected with |
|---|---|
| Paraphrase Criterion 4 in the contract | `F28C-001 ROADMAP Success Criterion 4: not quoted verbatim` |
| Strip KR-01's Phase 28 re-score | `F28L-006`, `F28L-015` |
| Reopen KR-05's accept path | `F28L-001` ×2, `F28L-007` ×2, `F28L-004`, `F28L-005` |
| Unbind the candidate commit from its tree | `commit is not bound to a tree` |
| Omit the `aarch64-apple-darwin` target | `omission is impossible by schema` |
| Blank an unbindable reason | `unbindable with no reason` |
| Tamper with `surface-capture.txt` | `sha256 mismatch — the candidate ledger no longer describes its inputs` |
| Drop every `offline`/`macos` cell | `F28M-005` |
| Skip a mandatory critical cell | `F28M-002`, `F28M-007` |
| Remove a mandatory cell | `F28M-007` |
| Observation-blocked skip citing the **favourable** intel file | `F28M-004 cites a document` |

`.planning/scripts/lint-plan-gates.py` over the phase directory: **4 plans, 75 gates, 0 HIGH,
0 other.**

Both validators carry `--self-test` that trips **every** rejection code with a bad fixture and
proves a good fixture does **not** trip it. `f28-ledger.py` covers 23 codes; the resolver covers
11 fail-closed conditions plus determinism plus `verify()` in both directions. One direction only
is how a validator ends up vacuous or indiscriminate — this program's own plan-gate linter shipped
that disease four separate times.

**One self-inflicted instance found and corrected during this run:** my first red-proof harness
printed `rc=0` for mutations that had plainly been rejected, because `$?` was taken after a
`| head -3` — the pipe stole the exit status, the exact shape the brief names. The harness was
wrong, not the gates; exit codes were re-measured without the pipe (`rc=1` on mutation, `rc=0` on
control). **The plan's own gates contain no pipe and were never affected.**

---

## 3. The candidate as resolved

| Field | Value |
|---|---|
| commit | `32e2f57d09fe4b287e513081862217dc9daa5901` |
| tree | `63ec0e6c36ff8e63789aab2f9760870304b671df` |
| **provisional** | **YES** |
| surface-probe binary | `sha256 da69ae6f7fac…` (full digest in `candidate.json`), `/root/wayland-p28/target/debug/wayland-core`, host `hetzner-dsm`, profile `debug` |
| KR-05 wedge repair `455dd836` | **PRESENT** — `git merge-base --is-ancestor 455dd836 32e2f57d` succeeds |
| surfaces discovered | **116** (24 depth-1 verbs + 92 depth-2 subcommands) |
| surfaces attributed to a phase | 67 of 116 |
| phases 24-27 with a verdict/summary artifact | **4 of 4** — 24: 5, 25: 4, 26: 2, 27: 5 |
| findings | 17 |

**Provisional reason, recorded in the artifact:** this is a pre-merge integration-branch tip, not
a released candidate. Lanes for phases 24, 26 and 23B were still executing when it was resolved.
**Plan 28-02 MUST re-resolve against the actual certification candidate**; the resolver is
parameterized precisely so that costs nothing.

Reproducibility is gate-checked: `candidate.json` carries **no timestamp**, records all 17 inputs
with their sha256, and `--verify-reproducible` re-resolves from exactly those and compares bytes
(54,974 bytes, identical).

---

## 4. STATED EXCEPTION — 0 of 6 targets bound, and what it costs

Every one of the six CI release targets carries an **explicit unbindable entry with a measured
reason**. None is omitted; omission is impossible by schema.

| Target | Status | Reason |
|---|---|---|
| `x86_64-unknown-linux-gnu` | unbindable | CI run `30269095004` at this exact commit is `status=queued` (measured 2026-07-27 via `gh run view`) |
| `aarch64-unknown-linux-gnu` | unbindable | same |
| `x86_64-apple-darwin` | unbindable | same |
| `aarch64-apple-darwin` | unbindable | same — **and this target IS obtainable from CI since `d9c7683b`**; it must be bound before the macOS leg is certified |
| `x86_64-pc-windows-msvc` | unbindable | same |
| `aarch64-pc-windows-msvc` | unbindable | same |

**Consequence for the matrix, stated plainly:** an unbindable target means that OS family's cells
**cannot be certified against a digest-bound artifact**. That is a stated limitation, not a silent
skip, and it is what termination state 2 exists to record. The reason is a **measurement**, not an
assumption — the CI run exists and was polled. Once it completes, re-running the resolver binds
all six with no code change.

**The claim "no macOS binary is obtainable" is FALSE and the resolver rejects it by name**
(`--self-test` fixture `the false 'no macOS binary is obtainable' claim`).

---

## 5. Findings

### Raised by the resolver — 17, all OPEN, all schema-valid against `f28-ledger.py --check-ledger`

| Class | Count | Phase 28 severity |
|---|---|---|
| `claimed-but-absent` | **0** | — |
| `present-but-unclaimed` | 10 | MEDIUM |
| `attribution-weak` | 7 | LOW |

`present-but-unclaimed`: `acp`, `agent`, `crucible`, `forge`, `init`, `mcp-serve`, `models`,
`project-context`, `self-update`, `swarm`. Each is exposed by the binary and appears nowhere in
any 24-27 artifact — most likely because they predate those phases, which is itself worth knowing
at certification time. **All are certified anyway**; an uncertified surface that ships is worse
than an unattributed one.

`attribution-weak` is a **third class I added, and the reason matters.** The first extractor
recognised only `wayland-core <verb>` in code spans and reported **19 of 24** verbs as unclaimed —
implausible on its face, and a limitation of the instrument rather than a fact about the product.
Phases actually write `` `gateway start` ``. Adding that bare form fixed recall but manufactured
**7 false `claimed-but-absent` findings** (`pid`, `pub`, `whenever`, `unrecognized`, …) from prose
and Rust fragments in backticks. The resolution is a precision/recall split recorded in the
artifact itself:

- the **explicit** form may accuse — it alone asserts `claimed-but-absent`, because accusing a
  phase of claiming a nonexistent surface costs a disposition to clear;
- the **bare** form attributes but never accuses;
- where neither recognises a claim for a surface the phase clearly *discusses*, the result is
  `attribution-weak`, **not** `present-but-unclaimed`.

That last rule is the standing "a measurement that cannot be taken must never render as `0`"
applied to attribution: a limit of the instrument is never rendered as a fact about the product.

### Raised by this plan against the record it encodes

**`F-28-01-003` — MEDIUM — the amendment commit explicitly disclaims Phase 28.** The decision's
load-bearing argument is that the severity amendment `d0837aa7` (2026-07-25) is the later
instrument and governs Criterion 4 (`0192e3c0`, 2026-07-19). **I re-ran that date check rather
than trusting the record; it is CONFIRMED.** But `d0837aa7`'s own commit message ends:

> Phase 28's criteria are untouched (different phase).

That is the amending instrument's author saying it does not reach Phase 28. It is the strongest
available evidence for the losing `c4-literal` position and it appears in **none** of
`decision-rationale.txt`, `decision-dissent.txt`, or the four captured panel responses.
**The decision was NOT reopened and nothing in the contract deviates from it** — it is recorded in
contract §9 so a later reader has the counter-evidence in hand. It does not by itself overturn the
decision: the sentence reads as consistently with "I am not editing that phase's text" as with
"that phase is exempt", and the dissent's own reversal condition is narrower. Under A2 the
practical gap is narrow anyway — the findings `c4-literal` would protect are exactly the ones A2
already removes from the accept path.

**`F-28-01-001` — MEDIUM — the unproven-control corollary, considered and deliberately NOT
applied.** A corollary of A2 would score an *unverified* Criterion-subject property on the
property, moving `KR-02` and `KR-03` across the A2 line — four crossings instead of two. The
decision record names two and the plan directs the contract to name two; inventing a stricter rule
than the recorded decision is what grew Phase 20 to 74 plans. Recorded in contract §3.4 with the
note that the structural rule reaches the same place by another road: both are plan-02 matrix
cells, and a cell that cannot produce positive evidence is a RED, and **where the inherited row
and the measured cell disagree the cell governs**.

**`F-28-01-002` — LOW — the standing severity policy is not in `AGENTS.md`.** The plan directed
quoting it verbatim from there; that file contains no such text
(`grep -i -E 'CRITICAL/HIGH|BACKLOG|non-blocking|disproved' AGENTS.md` → nothing). The contract
quotes `.planning/ROADMAP.md` and `d0837aa7` instead and says so in §1.3. → BACKLOG.

**No CRITICAL or HIGH finding was raised by this plan.**

---

## 6. The observability question — recorded as MEASURED-ON-ONE-HOST, not inherited

`.planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md` **existed at execution time**, and so does a
second file the plan did not know about, `.planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md`.
I read both and verified their claims against the tree rather than trusting a summary.

**What they say.** The standing rule — that a session-0 SSH logon reports AppContainer unavailable
regardless of correctness, so Windows sandbox reds from SSH are artifacts — is **REFUTED**.
`live_fs_acl` is **12/12 PASS** over a session-0, non-interactive SSH logon on a clean lease
directory at `455dd836`, *including `granted_path_is_readable_then_revoked`, the exact test the old
rule cited as its control*. The real cause was a stale lease written by `wcore-sandbox`'s **own
test suite** into the **production** lease directory, under which the product ran **unsandboxed**
while logging "sandbox disabled". The original control varied the *logon* while the lease directory
was wedged, so both hypotheses predicted its result — it never had discriminating power.

**How this plan treats it — and this is the part that matters.**

1. `KR-06` carries status **"lore REFUTED on one host; generalization to the certification
   environment OPEN."** Both halves are load-bearing. The intel says so itself: *"I did not test
   other Windows hosts... the observation is one box."*
2. **No rule, gate or skip in this plan has "sandbox reds from SSH are artifacts" as a passing
   condition.** A Windows sandbox red observed over SSH is now **evidence**.
3. **The refutation is equally unusable as skip evidence.** Contract §4.2 forbids citing *either*
   intel file by name, and `F28M-004` and `MatrixError::ObservationBlockedCitesDocument` enforce it.
   Proved red against the favourable file. **A laundering channel does not become sound by pointing
   it at good news** — that is the single design decision I would most want a reviewer to check.
4. The question occupies its own **mandatory critical cell**, `w-sandbox-observability-control`, so
   its answer is recorded as measured evidence in either direction.
5. **The observability question is OPEN and is settled by control in plan 28-02, not inherited
   here.** Nothing in this plan depends on either answer.

The lease wedge and the silent-disable defect may be two faces of one underlying defect. The matrix
is built to **distinguish** them: activeness is measured per cell, and the control has its own cell.

`KR-05`'s repair landed at `455dd836` and the resolver confirms it **IS** in the candidate. KR-05
is not closed by that — plan 28-02 must measure it.

---

## 7. The emitted matrix

**651 cells** = 9 dimensions × 3 OS families × **24 depth-1 surfaces** + 3 mandatory.

| Breakdown | |
|---|---|
| by criticality | **147 critical**, 504 standard |
| by OS family | linux 216, macos 216, windows 219 |
| by applicability | **651 applicable, 0 skipped** |
| per dimension | 72 each, except `sandbox-probes` 74 and `process-cleanup` 73 (the mandatory cells) |
| activeness required | 74 (every sandbox-probes cell, not only the mandatory one) |

**Zero skips, and that is correct rather than convenient:** no control has run, so the
observation-blocked class is unusable; no surface is claimed-but-absent, so unresolved-surface is
unusable; and all nine dimensions apply on all three families.

**The three mandatory cells are present, `critical`, and `applicable`** —
`w-sandbox-silent-disable`, `w-process-cleanup-descendant-tree`, `w-sandbox-observability-control`.

**Two scope decisions the orchestrator should see, both stated rather than silent:**

1. **`max_surface_depth=1`** is recorded in the TSV header. The 92 depth-2 surfaces remain in
   `candidate.json` and plan 28-02 may expand to them. Generating at depth 2 would produce ~3,135
   cells.
2. **Criticality is declared per (dimension, OS family)** in a table, never per cell — deciding it
   per cell is how a critical case becomes non-critical the moment it is inconvenient. Two
   dimensions are critical on every family because each is the literal subject of a Success
   Criterion: `sandbox-probes` (Criterion 1) and `process-cleanup` (Criterion 2). **This makes 147
   cells unskippable, which is a real workload for plan 28-02.** I am flagging it rather than
   quietly narrowing it: narrowing would be softening Criterion 1.

---

## 8. Scope and honesty statements

- **The acceptance decision was NOT reopened.** It is encoded. One piece of counter-evidence found
  while encoding it is recorded as `F-28-01-003` and acted on in no way.
- **No 24-27 surface is named anywhere in this plan's authored artifacts.** Every surface in
  `candidate.json`, `28-01-CANDIDATE-LEDGER.md` and `matrix.tsv` is **resolver output read off the
  binary**. The contract, both scripts, `e5_matrix.rs` and the contract test name none; the test's
  fixtures use `cmd:alpha` / `cmd:beta`.
- **No rule or gate depends on Windows sandbox reds from SSH being artifacts.**
- **Nothing was executed, soaked or signed.** No matrix cell was run.
- **No production file outside `crates/wcore-eval-scenarios` was touched** —
  `git diff --name-only 32e2f57d..HEAD -- crates/` returns exactly `src/e5_matrix.rs`,
  `src/lib.rs`, `tests/e5_matrix_contract.rs`.
- **No new crate, no new dependency, NO `Cargo.toml` or `Cargo.lock` change.** `wcore-eval-scenarios`
  already existed and already depended on `serde_json` and `thiserror`. **There is no shared-file
  fence to serialize for this lane.**
- **No existing test was modified, renamed, re-gated, `#[ignore]`d, `#[allow]`ed or deleted.** The
  single clippy failure (`collapsible_if`) was fixed by collapsing the branch, not by allowing it.
- `wcore-contract generate` was **not** run. No PR, merge, tag or release.
- Phase 28's four-plan cap is intact: **no fifth plan was created or proposed.**

## 9. Deviations

1. **Standing severity policy quoted from `ROADMAP.md` + `d0837aa7`, not `AGENTS.md`** — it is not
   in `AGENTS.md`. Recorded as `F-28-01-002` and stated in contract §1.3.
2. **A third resolver finding class, `attribution-weak`** — see §5. Added to stop a limitation of
   the extractor being reported as a property of the product.
3. **The plan says "both mandatory cells" in one sentence and "three" in four others.** Built
   three, per the behaviour block and the success criteria.
4. **The candidate is provisional** — unavoidable: the real candidate does not exist yet. This is
   the plan's own central design constraint, not a shortfall.

## 10. What plan 28-02 must do first

1. **Re-resolve the candidate** against the actual certification commit. Re-run `--verify` and
   `--verify-reproducible` before spending hardware time.
2. **Bind the six per-target digests** once the CI release matrix completes. Until then no OS
   family is certified against a digest-bound artifact.
3. **Measure the Windows sandbox observability control** in the certification environment, before
   grading any sandbox cell. Neither the lore nor its refutation is admissible.
4. **Obtain a requirement disposition from the owning phase** before any `unresolved-surface` skip.
5. **Decide `max_surface_depth`** deliberately and record it.
