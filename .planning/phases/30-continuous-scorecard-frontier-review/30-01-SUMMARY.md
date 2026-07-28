---
phase: 30-continuous-scorecard-frontier-review
plan: "01"
subsystem: scorecard / competitive-ledger review / surface inventory
status: complete
termination_state: 1 (Complete)
requirements: [F30-01, F30-02]
requirements_claimed: []
lane_branch: lane/30-01
review_base: eab69cdbc244cfe90b0a623a9fb15c80da249d24
proof_sha: 4f749251060a0b22546dd6341c82a5e049083237
completed: 2026-07-28
tags: [scorecard, ctrl-01-review, surface-inventory, closed-enum, asymmetric-verdict]
---

# Phase 30 Plan 01 — Scorecard, CTRL-01 review and surface inventory — Summary

Built the instrument Phase 30 grades everything else with, proved it can fail,
reviewed CTRL-01 mechanically, and walked the surface inventory out of a real
release binary. **No requirement marked complete; nothing positioned.**

**Termination state 1 (Complete.)** No fourth state was invented.

## Commits and SHAs

| | |
|---|---|
| Lane branch | `lane/30-01` (pushed to `gh`) |
| Merge base / review base | `eab69cdbc244cfe90b0a623a9fb15c80da249d24` |
| SHA all Hetzner proofs ran at | `4f749251060a0b22546dd6341c82a5e049083237` |
| Hetzner worktree | `/root/wayland-30-01` (detached; the shared `/root/wayland` was **not** checked out — other lanes share it) |
| `wayland-core` release sha256 | `e73453a5ba0cdb23ace670106de036d24e28fc6ed6f38202a842c87a86c5aaae` |

## 1. The instrument was watched failing — twice, two different instruments

This is the thing the phase is most exposed to, so it is first.

**The Python review instrument** (`evidence/30-01/mutation-harness.sh`) applies four
mutations to a **copy of the real ledger** — never the real file — plus one control:

```
BASELINE (pristine real ledger)     CONFIRMED PARTIAL UNRESOLVED = 39 1 2
PASS  M1 path repointed at a nonexistent file      : 39 1 2 -> 38 1 3
PASS  M2 one flipped byte in a pinned commit SHA   : 39 1 2 -> 38 2 2
PASS  M3 fabricated evidence-ID row appended       : 39 1 2 -> 39 1 3
PASS  M4 altered artifact sha256 digest            : 39 1 2 -> 38 2 2
PASS  C1 CONTROL prose reworded, no citation touched: 39 1 2 -> 39 1 2 (unchanged)
real ledger unmodified check: 0 modified paths
SEPARATION_RESULT=PASS
```

**The Rust instrument**, against the real shipped binary
(`evidence/30-01/live-verify-transcript.txt`):

| Run | Input | Exit | Output |
|---|---|---|---|
| 1 | pristine doc — one criterion `NOT_MET` with an **empty** evidence set, one `MET` with resolving evidence | **0** | `SCORECARD_VERIFY=OK criteria=2 surfaces=0` |
| 2 | same doc, MET evidence repointed at a nonexistent path | **1** | ``criterion `SC-B` is graded MET but evidence reference `EV-REAL` does not resolve`` |
| 3 | same doc, grade replaced with `ready_for_frontier_positioning` | **1** | ``unknown variant … expected one of `MET`, `MET_WITH_STATED_EXCEPTIONS`, `PARTIAL`, `NOT_MET`, `UNPROVEN` `` |

Run 1 accepting a `NOT_MET` with an empty evidence set **is** the asymmetry: the
honest grade cost nothing while the confident one cost proof.

**And the instruments were caught carrying the defect they hunt — five times, all
before shipping.** Only reading the output caught each:

1. `git cat-file` run on truncated **sha256 build digests** → 3 false UNRESOLVEDs.
2. `[0-9a-f]{8,40}` matching **decimal workflow-run ids** → 1 more.
3. Blindness to the ledger's bare-filename sibling shorthand → 1 more.
4. `test -f` treating a SUMMARY's **existence** as execution — misclassified
   `23A-02`, which exists precisely to record a non-start.
5. A `set -- $spec` word-split idiom that **silently yields one field in zsh**,
   producing a peer-baseline report that printed `AGREES` having run nothing. It hit
   twice; the second time the review ledger's own NO-MALFORMED-ROW gate caught it
   (60 prefixed rows vs 52 well-formed).

Every correction **sharpened** the instrument. None loosened a rule.

## 2. CTRL-01 review — `30-01-LEDGER-REVIEW.md`

60 determinations in `evidence/30-01/LEDGER-REVIEW.tsv`, each naming a captured file.

- **42 evidence IDs** (the plan was calibrated at 8; the 2026-07-28 refresh added 34):
  **39 CONFIRMED, 1 PARTIAL, 2 UNRESOLVED**.
  - `PEER-PROBE-2026-07-26` — **HIGH**, names no openable artifact at all, yet
    carries half the Delta column in six families.
  - `F05-TRUTH-{n}` — LOW, a template ID; its concrete instances resolve.
  - `F28-MATRIX-651` — LOW, cites `results.json` without a path; the file is real.
- **Both pinned peer baselines re-verified read-only** — commit resolves, ancestry
  holds, version read back at the pinned commit: Hermes `0.17.0`
  (`pyproject.toml:10`), OpenClaw `2026.6.2` (`package.json:3`). Both AGREE. Both
  trees left clean; **no write verb used in either**.
- **All ten families pass all seven required clauses.** Zero defects, zero
  undeclared evidence IDs, zero `UNPINNED`. Reported clean, not inflated.
- **Thirteen tracking-document claims falsified by the tree; two controls held.**

**The staleness runs in the direction nobody checks for — it UNDERSTATES the
program.** PORT-\* asserts "the entire import half is unbuilt" (26-02 and 26-04 are
`status: complete`); REACH-\* asserts three unmet criteria and two Sean-reserved
blockers (`25-PHASE-STATUS.md` now carries **four bolded METs**, the cloud credential
minted, the second host closed). The ledger was refreshed at 08:28; those lanes
landed the same day. It is **dated, not wrong** — but a reader cannot tell, and
Phase 30 must not position from its Limitation columns as written.

**The ledger was not edited.** Re-grading is the row owner's action, not the
reviewer's — that is the point of the read-only fence.

## 3. The scorecard module — `crates/wcore-eval-scenarios/src/scorecard.rs`

- `MaturityV1`: closed, the ledger's eight states, tokens spelled per variant.
  `ALL_MATURITY_STATES: [MaturityV1; 8]` makes the **compiler** check the count.
- `CriterionVerdictV1`: closed at five. `ready_for_frontier_positioning` appears
  **only** in the test that rejects it — gate-checked absent from the module.
- `SurfaceRowV1`: all seven truths REQUIRED. `deny_unknown_fields` at five boundary
  structs.
- **The asymmetry:** `MET`/`MET_WITH_STATED_EXCEPTIONS` require non-empty evidence,
  every reference resolving, none unproven — each failure a distinct typed error
  **naming** the reference. `NOT_MET`/`PARTIAL`/`UNPROVEN` require nothing and always
  succeed.

**One design change the inventory forced.** Six shipped commands are owned by no
family, so no maturity has ever been recorded for them. `MaturityV1` has no member
meaning "nobody graded this" and must not grow one — `ABSENT` would assert the
capability does not exist, which is false. The unmeasured case was lifted **out** of
the enum into `MaturityTruthV1`, so the closed enum keeps refusing undeclared tokens
while "not yet graded" stays sayable. A test asserts
`{"state":"measured","value":"UNPROVEN"}` is still refused.

## 4. Surface inventory — `30-01-SURFACE-INVENTORY.md`

**148 surfaces, 28 top-level commands** (the plan was calibrated at 21), walked out
of the real release binary. **Regeneration: the shipped walker re-run against the
same sha256-asserted binary produced a table byte-identical (`diff -u` clean, 149
lines) to the committed `evidence/30-01/surfaces.tsv` in this git tree.** Not
cross-host determinism — one Linux host, one binary; that limit is stated.

Buckets: BINARY_AND_DOCS 42, BINARY_ONLY 91, DOCS_ONLY 4 (1 real, 3 noise),
NO_FAMILY 15 rows / 6 top-level commands.

Two findings, both from running rather than reading:
- **Six shipped top-level commands are owned by no family** (`init`, `mcp-serve`,
  `models`, `profile`, `project-context`, `setup`) — no security owner, no maturity,
  no peer baseline. Three are first-run/credential-adjacent. MEDIUM.
- **Clap aliases are invisible to `--help`.** `wayland-core forgeflows list` runs
  (rc=0, real output) but has no inventory row. The docs are right; **my walker is
  incomplete.** It measures the *advertised* tree, not the *accepted* surface.
  MEDIUM, recorded as a ceiling on this plan's strongest artifact.

**`peer_delta` is UNPROVEN on all 148 rows** because no comparative trial has run.
30-02 owns it; a number here would forge the figure the phase exists to earn.

## 5. Gates — read, not assumed

Local (Mac), all GREEN: `cargo fmt --all -- --check`; MODULE-LANDED; CLOSED-VERDICT;
CLOSED-MATURITY; CLOSED-SHAPE (≥3, actual 5); NAMED-BEHAVIOR; INVENTED-GRADE; FENCE;
INVENTORY-PRESENT; THREE-WAY-DIFF (≥21, actual 152); NO-GUESSED-TRUTH;
INVENTORY-VERIFIER; RESOLVED-EVIDENCE (≥18, actual 60); LEDGER-UNMODIFIED;
PEER-PIN; NO-MALFORMED-ROW (60 == 60).

**Two gates RED, both from stale calibration against a moved tree, neither hiding
work — and neither "fixed" by editing the tree:**
- `LEDGER-STRUCTURE` expects 20 family / 8 evidence rows; measured **26 / 42**.
- `BINARY-DECLARED` expects `[[bin]] == 4`; measured **7** (Phases 24 and 29 added
  three since calibration). Re-scoped to `== 7` plus the name check: GREEN.

The plan's own gate comment sanctions exactly this: *"the count moves and the review
is re-scoped rather than silently stale."*

**Hetzner (`hetzner-dsm`, targeted, 757G free, load 2.46):**
- `cargo nextest run -p wcore-eval-scenarios --no-fail-fast` → **456 tests run, 456
  passed, 5 skipped, rc=0.** All 12 scorecard contract tests PASS.
- `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` → **RED, 4
  errors, rc=101. Zero are mine.** All four are `cloned_ref_to_slice_refs` in
  `crates/wcore-eval-scenarios/src/journey.rs:{683,695,707,717}`, a Phase 24 file.
  **Proved pre-existing by measurement, not asserted:** a second worktree at base
  `eab69cdb` with my changes absent produces the identical 4 errors. Delta: 4 → 4,
  zero added. Reported red and attributed; **not fixed** (surgical-diff discipline,
  another lane's file), **not silenced** (no `#[allow]`).
- Live: `wayland-scorecard verify` ×3 and the regeneration diff (§1, §4).

**Honest RED-before-GREEN:** the first Hetzner run of the suite was
**455/456 with `every_surface_row_in_the_committed_inventory_deserializes_and_verifies`
FAILING**, because Task 3's inventory did not yet exist. It went green only once the
real inventory landed. Recorded rather than hidden.

## 6. Not done / not claimed

- **No requirement marked complete.** F30-01 and F30-02 carry evidence only;
  closure is 30-04's, once, against the four criteria verbatim.
- **Nothing positioned, nothing recommended.** That decision is Sean's.
- **No re-grading of any ledger row**, though PORT-\* and REACH-\* are evidenced
  candidates for promotion. Reviewer ≠ author.
- **No credential of any kind** read, requested, printed, logged or committed. No
  gate here needs one.
- **No peer-tree write.** Read-only verbs only; both trees left clean.
- **Not measured:** macOS/Windows command trees; cross-host walk determinism; clap
  alias surface; whether the ten families are the right partition.
- **No test weakened, deleted, `#[ignore]`d, `#[allow]`ed or re-gated.**
- Untouched and gate-checked: `COMPETITIVE-LEDGER.md`, `REQUIREMENTS.md`,
  `receipt.rs`, `receipt_policy.rs`, `wayland-receipt.rs`, `fixtures/openai.rs`,
  `wcore-cli/src/{lib,main}.rs` (**0 lines vs merge-base**), root `Cargo.toml`,
  `Cargo.lock`. No `wcore-contract generate`.

## 7. Findings

**CRITICAL: none. HIGH: 6** — `PEER-PROBE-2026-07-26` unresolvable; STALE-01/02
(PORT-\*) and STALE-11/12/13 (REACH-\*) materially understating the tree.
**None blocks 30-02.** **MEDIUM: 8. LOW: 2.**

**Verdict: the plan's criteria are met.** The one thing a reader should carry
forward: **Phase 30 must not position from CTRL-01's Limitation columns as written** —
they are ~13 hours stale and understate the program in five measured places.
