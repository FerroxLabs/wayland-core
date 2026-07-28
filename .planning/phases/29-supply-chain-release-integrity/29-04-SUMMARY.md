---
phase: 29-supply-chain-release-integrity
plan: "04"
subsystem: supply-chain / tamper corpus, release-state separation, phase verdict
termination_state: 2 (Complete, goal not achieved)
status: complete
completed: 2026-07-28
tags: [tamper-corpus, paired-control, release-states, separation, phase-verdict, seam-request]
requires:
  - 29-01 (release manifest, role-scoped trust root, closed four-state ledger)
  - 29-02 (SBOM, dependency policy, reproducibility)
  - 29-03 (update trust path)
provides:
  - the paired tamper corpus over all seven F29-03 object classes
  - the two four-state separation runs, driven through the real binary
  - the Phase 29 verdict, four criteria quoted verbatim and graded
affects:
  - Phase 25 (runtime plugin/backend trust roots — SR-29-8)
  - Phase 28 (R28-A..R28-F receipt requirements — SR-29-1..SR-29-5)
  - Phase 30 (independent review inherits four PARTIAL grades and two open HIGHs)
tech-stack:
  added: []
  patterns: [structurally-required-control, corpus-over-real-verifier, verbatim-grading]
key-files:
  created:
    - crates/wcore-eval-scenarios/tests/supply_chain_tamper_corpus.rs
    - .planning/phases/29-supply-chain-release-integrity/29-04-TAMPER-RESULTS.md
    - .planning/phases/29-supply-chain-release-integrity/29-PHASE-VERDICT.md
    - .planning/phases/29-supply-chain-release-integrity/evidence/29-04/
  modified:
    - .planning/SEAM-REQUESTS/29.md
    - .planning/BACKLOG.md
decisions:
  - "A tamper case builds its own pristine control, so a case without one cannot be constructed"
  - "No case asserts a rendered error string; each asserts only that the input was refused"
  - "The wayland-receipt CI-sign refusal was recorded as a RESULT and not bypassed"
  - "All four Success Criteria graded PARTIAL; no requirement marked complete"
metrics:
  tamper_cases: 12
  object_classes: 7
  negative_controls: 4
  collapse_attempts: 7
  standalone_verifications: 4
  cross_role_controls: 12
  production_files_modified: 0
---

# Phase 29 Plan 04: Tamper Corpus, State Separation and the Phase Verdict — Summary

**Phase 29's goal was not achieved. All four Success Criteria grade PARTIAL.** That is the
first line of `29-PHASE-VERDICT.md` and it is the first line here, because it is the answer.

**Termination state: 2 — Complete, goal not achieved.** The plan names that state as a full
and successful outcome. No fourth state was invented, no plan was spawned, no criterion was
graded against a reworded version of itself, and no production source was modified anywhere.

Everything executable was measured on `hetzner-dsm` at
`f6196a3275d41eaf769d4271b38d145e18739f26`; the documents and ledgers were graded at
`882191d4737c7552bef33214f4aadc31dbf828a3`. Lane `lane/29-04`, off
`plan/f20-unified-audit-repair` at merge base `c743f398`.

---

## What landed

| Artifact | What it is |
|---|---|
| `crates/wcore-eval-scenarios/tests/supply_chain_tamper_corpus.rs` | 12 paired cases across 7 object classes. **The only file this plan added under `crates/`.** |
| `29-04-TAMPER-RESULTS.md` | Per-case accept-then-refuse table, both four-state runs, the collapse attempts, the findings |
| `29-PHASE-VERDICT.md` | The four criteria quoted verbatim and graded, both HIGHs disposed of, eight limits enumerated |
| `evidence/29-04/` | 6 ledgers + 20 captured artifacts |
| `.planning/SEAM-REQUESTS/29.md` | SR-29-6 amended; SR-29-13/14/15 added; SR-29-9/11 reaffirmed; SR-29-12 disposed |
| `.planning/BACKLOG.md` | Four MEDIUM findings, non-blocking |

## RED before GREEN — the corpus was proved able to fail

A corpus that passes proves nothing until it has been shown to fail. Four defects were
introduced one at a time into the corpus itself, each run, each restored from a copy held
outside the worktree (`evidence/29-04/tamper-negative-controls.txt`):

| Control | Defect | Result |
|---|---|---|
| NC-1 | one case's mutation made a no-op | **rc=101** — `Paired::new` refuses identical halves |
| NC-2 | pristine manifest signed with the wrong role key, so every control is refused | **rc=101** — this is exactly the rejection-only shape a verifier that refuses everything would pass |
| NC-3 | the SBOM case deleted from the table | **rc=101** — class-coverage meta-assertion fires |
| NC-4 | one mutation replaced with a change the verifier ignores | **rc=101** — the refusal half is live |

`dirty_paths_under_crates=0` afterwards; `restored_rc=0`.

## The 12 cases — 12 controls accepted, 12 mutations refused

Full table with the exact mutation per case: `29-04-TAMPER-RESULTS.md`. Zero cases whose
result was not accept-then-refuse, so **no case became a finding**. Classes: binary, SBOM,
update ×2, plugin, backend-receipt ×2, manifest ×2, key ×3.

Two results worth naming. `F29-03-BACKEND-RECEIPT-2` leaves the manifest completely untouched
and mutates only the bound receipt, so `verify_manifest` still passes and the refusal comes
from the **join** — that is the only case that proves the join exists. `F29-03-KEY-3` replays
a signature the **same** key minted over the **same** body digest under a different domain
separator; only the domain differs.

## Both four-state runs, through the real binary

| Run | Outcome |
|---|---|
| **A-1** — all four keys, shipped tool only | appends all rc=0, `state-verify` **rc=1**: `release acceptance requires an observed certification binding`. Holding every key is not sufficient. |
| **A** — all four keys, Phase 28 seam instantiated in the clean room | `CHAIN VERIFIED highest_state=release_acceptance records=4 accepted=true` |
| **B** — release-acceptance key withheld (3 keys, 3 seeds) | **rc=0**, `highest_state=rollback_rehearsal records=3 accepted=false` — a stopping point, not a corrupt chain |

Standalone: each of the four records verified by **OpenSSL 3.0.13** against only its own role
key, paired with **12 cross-role controls all failing**. Seven collapse attempts — relabel,
wrong-role signature, reused evidence, skipped state, reordered chain, an invented fifth state
named `termination_state_4` (refused at both the CLI parser and at deserialization), and
acceptance signed by a freshly rotated *packaging*-role key — every one refused with a capture.

**The mechanism is proved. No release was accepted.** The key that completed run A was
generated at run time into a temp directory and died with the run.

## The CI-sign refusal is a RESULT, not an obstacle

`wayland-receipt sign` returned **rc=1** —
`fixture digest is a synthetic binary/scenario label, not fixture provenance`. The product
refuses to mint a CI authority claim over a receipt whose fixture provenance is synthetic.
**That guard firing exactly when it should is the behaviour this phase set out to prove, and
it was not bypassed** — minting the signature by hand would have defeated the control. The
clean-room binding therefore names a LOCAL-authority receipt and says so in its own
`receipt_signing_key_id` field. **NOT PROVABLE HERE**, substitution point: a CI-signed receipt
from a real evaluation run (F29-LIMIT-07).

## Deviation, disclosed

`wayland-release manifest build` hardcodes `certification: Evidence::Unavailable` with no flag
to record a binding, so run A's manifest body was assembled outside `manifest build` from a
receipt the real evaluation pipeline emitted, then signed and verified **through the real
binary** — `manifest-verify rc=0` is the self-check that the recomputed body digest is
byte-exact. Recorded as finding **F29-04-01**, not repaired: this plan modifies no production
source, because a corpus written by the hand that fixed the defect proves only that its author
knew what they fixed.

## The verdict — four criteria, graded verbatim

| # | Criterion (abbreviated; quoted verbatim in the verdict) | Grade |
|---|---|---|
| 1 | clean-room builds verify provenance, SBOM, dependency policy, signatures, reproducibility or documented variance | **PARTIAL** — SBOM and reproducibility MET; the dependency-policy verdict is **FAIL exit 5**; the provenance ACCEPT path has never been observed |
| 2 | install/update paths verify identity, rollback/freeze, revocation, key rotation | **PARTIAL** — rollback and rotation MET (rollback measured live through the shipped binary against the real public API); the update path installs **nothing** today |
| 3 | tampered artifacts, manifests, receipts, plugins, backends or keys are rejected | **PARTIAL** — artifacts, manifests and keys MET; plugins and backends covered only at the release-manifest layer, their runtime trust root still an all-zeros placeholder |
| 4 | the four states remain separate evidence and authorization states | **PARTIAL** — separation proved comprehensively as a mechanism; in the shipped pipeline **zero** approval gates, **zero** rollback steps and **zero** ledger invocations, so one tag push drives all three shipped stages |

**Requirements F29-01, F29-02, F29-03 and F29-04 are each NOT COMPLETE.** No requirement was
marked complete on the strength of a PARTIAL criterion.

## The two open HIGHs — neither closed, neither mine to accept away

- **F29-02-H1** — **OPEN.** Re-measured independently (`evidence/29-04/open-high-f29-02-h1.txt`).
  **Sustained:** `.cargo/audit.toml` claims a "sole path" and UNREACHABLE while
  `wcore-tools 0.12.25` depends on **quick-xml 0.39.4 directly**, behind the **default-on**
  `doc-extract` feature, to parse user-supplied docx/pptx. **One leg withdrawn:**
  `calamine 0.26.1` resolves to quick-xml **0.31.0**, which neither advisory names, so its 25
  `.attributes()` sites do not land on the affected version — the finding does not depend on
  it. 0195's UNREACHABLE claim continues to hold. **Criterion 1 cannot be MET while this is
  open.** Escalated via SR-29-6; `.cargo/audit.toml` is fenced out of this plan.
- **F29-03-01** — **OPEN.** `self-update` installs nothing until a real trust root is
  substituted (SR-29-11) **and** releases publish a manifest asset (SR-29-9). Deliberate
  fail-closed *and* a broken update path — both true. **Criterion 2 cannot be MET.** Not
  closable by any agent.

## Findings opened here — all MEDIUM, all filed, severity policy not tightened

`F29-04-01` (no tool path to record a binding) · `F29-04-02` (acceptance gates on `Observed`,
not on a verified join) · `F29-04-03` (the ledger is not wired into `release.yml` — the reason
criterion 4 is PARTIAL) · `F29-04-04` (**correction**: `seandesktop` is reachable as `SeanD`;
29-03's blocker is falsified — the Windows leg was **not** run, a concurrent lane holds that
host, **serialize**).

## Gates — read, not assumed

15 local gates PASS (fmt, corpus-landed, seven-class, paired-result, no-error-string-pinning,
two-run, collapse-attempt, invented-state, standalone-verifiability, verdict-graded,
verbatim-quote, no-release-accepted, phase-limits, residuals-filed, no-production-change).

Hetzner: `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` **exit 0**;
`cargo nextest run -p wcore-eval-scenarios --no-fail-fast` → **376 run, 376 passed, 5 skipped,
exit 0**, zero residual failures. An **isolated single-crate** run, stated as such: a
full-workspace run taken while other lanes build is not a measurement.

**Two measurement traps caught in this plan's own gates**, recorded because both are the
self-passing class:
1. `cargo fmt --all -- --check | head` reported `FMT_RC=0` while the diff was non-empty — the
   pipe stole the exit status. Re-run without a pipe.
2. The VERBATIM-QUOTE gate read **`0`** for two fragments that were present verbatim: both
   were **split across a line wrap**, and `grep -cF` is line-oriented. Fixed by quoting each
   gated fragment on one unwrapped line. A gate that reads 0 for text that is there is exactly
   as dangerous as one that reads 1 for text that is not.
3. The plan's `NO-PRODUCTION-CHANGE` gate uses `git status --porcelain`, which is **vacuous
   once the work is committed**. Re-run in merge-base form against the captured
   `c743f398`: exactly **1** path under `crates/`, it is the whitelisted one, **0** lines of
   diff on the shared fence (`lib.rs`, `main.rs`), **0** deletions anywhere.

## Recorded unknowns — stated, not resolved

Whether release-manifest-layer coverage of plugins and backends satisfies F29-03 for an
independent reviewer; whether the ledger's evidence-disjointness rule is the right strength
for a real release where evidence legitimately spans states; and whether these grades can be
re-graded upward once Phase 25 and Phase 28 land their halves. That last is Phase 30's call,
not this phase's to pre-empt.

## The irreducible limit

The ledger files were written by this executor, and no gate can verify that a captured
transcript was produced by the command it claims. The mitigations are structural: the corpus
is **run** by a remote gate rather than read about, every ledger row names an artifact a gate
stats, the paired-result gate fails on the exact shape a fabrication would take
(`control=REFUSED`), and both four-state runs are reproducible from the retained driver script
against the same commit.
